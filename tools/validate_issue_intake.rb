#!/usr/bin/env ruby
# frozen_string_literal: true

require "optparse"
require "set"
require "yaml"

module IssueIntake
  REQUIRED_FORMS = {
    "bug.yml" => "kind:bug",
    "compatibility_regression.yml" => "kind:compatibility",
    "enhancement.yml" => "kind:enhancement",
    "agent_wedge.yml" => "kind:wedge"
  }.freeze
  REQUIRED_DIMENSIONS = %w[kind area priority status environment].freeze
  ALLOWED_BODY_TYPES = %w[markdown input textarea dropdown checkboxes].freeze
  COLOR = /\A[0-9a-fA-F]{6}\z/.freeze

  class Invalid < StandardError; end

  module_function

  def load_yaml(path)
    YAML.safe_load(
      File.read(path),
      permitted_classes: [],
      permitted_symbols: [],
      aliases: false
    )
  rescue Errno::ENOENT
    raise Invalid, "missing required file: #{path}"
  rescue Psych::SyntaxError => error
    raise Invalid, "invalid YAML in #{path}: #{error.message.lines.first.strip}"
  end

  def validate!(root:, live_labels_path: nil)
    errors = []
    template_dir = File.join(root, ".github", "ISSUE_TEMPLATE")
    label_path = File.join(root, ".github", "labels.yml")
    label_document = load_yaml(label_path)
    label_entries = label_document.is_a?(Hash) ? label_document["labels"] : nil

    unless label_entries.is_a?(Array) && !label_entries.empty?
      raise Invalid, "#{label_path}: top-level labels must be a non-empty array"
    end

    names = []
    label_entries.each_with_index do |entry, index|
      unless entry.is_a?(Hash)
        errors << "#{label_path}: labels[#{index}] must be a mapping"
        next
      end

      name = entry["name"]
      names << name if name.is_a?(String)
      errors << "#{label_path}: labels[#{index}].name must be a non-empty string" unless name.is_a?(String) && !name.empty?
      errors << "#{label_path}: #{name || "labels[#{index}]"} needs a six-digit hex color" unless entry["color"].is_a?(String) && COLOR.match?(entry["color"])
      errors << "#{label_path}: #{name || "labels[#{index}]"} needs a description" unless entry["description"].is_a?(String) && !entry["description"].strip.empty?
    end

    duplicates = names.group_by(&:itself).select { |_name, occurrences| occurrences.length > 1 }.keys
    errors << "#{label_path}: duplicate labels: #{duplicates.sort.join(", ")}" unless duplicates.empty?

    REQUIRED_DIMENSIONS.each do |dimension|
      errors << "#{label_path}: missing #{dimension}:* label dimension" unless names.any? { |name| name.start_with?("#{dimension}:") }
    end

    known_labels = names.to_set
    REQUIRED_FORMS.each do |filename, kind_label|
      validate_form(
        File.join(template_dir, filename),
        kind_label,
        known_labels,
        errors
      )
    end

    validate_config(File.join(template_dir, "config.yml"), errors)
    validate_live_labels(live_labels_path, known_labels, errors) if live_labels_path

    raise Invalid, errors.join("\n") unless errors.empty?

    true
  end

  def validate_form(path, kind_label, known_labels, errors)
    form = load_yaml(path)
    unless form.is_a?(Hash)
      errors << "#{path}: form must be a mapping"
      return
    end

    %w[name description title].each do |key|
      errors << "#{path}: #{key} must be a non-empty string" unless form[key].is_a?(String) && !form[key].strip.empty?
    end

    labels = form["labels"]
    expected_labels = [kind_label, "status:triage"].to_set
    unless labels.is_a?(Array) && labels.to_set == expected_labels && labels.length == expected_labels.length
      errors << "#{path}: default labels must be exactly #{expected_labels.to_a.sort.join(", ")}"
    end
    Array(labels).each do |label|
      errors << "#{path}: referenced label is absent from .github/labels.yml: #{label}" unless known_labels.include?(label)
    end

    body = form["body"]
    unless body.is_a?(Array) && !body.empty?
      errors << "#{path}: body must be a non-empty array"
      return
    end

    ids = []
    body.each_with_index do |element, index|
      unless element.is_a?(Hash)
        errors << "#{path}: body[#{index}] must be a mapping"
        next
      end

      type = element["type"]
      errors << "#{path}: body[#{index}] has unsupported type #{type.inspect}" unless ALLOWED_BODY_TYPES.include?(type)
      next if type == "markdown"

      id = element["id"]
      ids << id if id.is_a?(String)
      errors << "#{path}: body[#{index}] needs an id" unless id.is_a?(String) && !id.empty?
      errors << "#{path}: body[#{index}] needs attributes" unless element["attributes"].is_a?(Hash)
    end

    duplicate_ids = ids.group_by(&:itself).select { |_id, occurrences| occurrences.length > 1 }.keys
    errors << "#{path}: duplicate body ids: #{duplicate_ids.sort.join(", ")}" unless duplicate_ids.empty?

    warning_text = body.map do |element|
      next unless element.is_a?(Hash) && element["type"] == "markdown"

      element.dig("attributes", "value")
    end.compact.join(" ").downcase
    %w[secrets private prompts log].each do |term|
      errors << "#{path}: safety warning must mention #{term}" unless warning_text.include?(term)
    end
  end

  def validate_config(path, errors)
    config = load_yaml(path)
    unless config.is_a?(Hash)
      errors << "#{path}: config must be a mapping"
      return
    end

    errors << "#{path}: blank_issues_enabled must be false" unless config["blank_issues_enabled"] == false
    links = config["contact_links"]
    security_link = Array(links).find do |link|
      link.is_a?(Hash) && link["url"].is_a?(String) && link["url"].include?("/security/advisories/new")
    end
    errors << "#{path}: contact_links must route security reports to a private advisory" unless security_link
  end

  def validate_live_labels(path, known_labels, errors)
    live_labels = File.readlines(path, chomp: true).reject(&:empty?).to_set
    missing = known_labels - live_labels
    errors << "live repository is missing labels: #{missing.to_a.sort.join(", ")}" unless missing.empty?
  rescue Errno::ENOENT
    errors << "live-label input does not exist: #{path}"
  end
end

if $PROGRAM_NAME == __FILE__
  options = { root: File.expand_path("..", __dir__) }
  parser = OptionParser.new do |opts|
    opts.banner = "Usage: tools/validate_issue_intake.rb [--root PATH] [--live-labels PATH]"
    opts.on("--root PATH", "Repository root (defaults to this script's parent)") { |path| options[:root] = File.expand_path(path) }
    opts.on("--live-labels PATH", "Newline-delimited live GitHub label names") { |path| options[:live_labels_path] = path }
  end
  parser.parse!

  begin
    IssueIntake.validate!(**options)
    puts "issue intake validation passed"
  rescue IssueIntake::Invalid => error
    warn error.message
    exit 1
  end
end
