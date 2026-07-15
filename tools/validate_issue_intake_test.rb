#!/usr/bin/env ruby
# frozen_string_literal: true

require "fileutils"
require "minitest/autorun"
require "tmpdir"
require_relative "validate_issue_intake"

class ValidateIssueIntakeTest < Minitest::Test
  ROOT = File.expand_path("..", __dir__)

  def test_repository_intake_is_valid
    assert IssueIntake.validate!(root: ROOT)
  end

  def test_malformed_form_yaml_is_rejected
    with_fixture do |root|
      File.write(File.join(root, ".github/ISSUE_TEMPLATE/bug.yml"), "name: [\n")

      error = assert_raises(IssueIntake::Invalid) { IssueIntake.validate!(root: root) }
      assert_includes error.message, "invalid YAML"
    end
  end

  def test_form_label_missing_from_catalog_is_rejected
    with_fixture do |root|
      labels = File.join(root, ".github/labels.yml")
      File.write(labels, File.read(labels).sub(/  - name: "kind:bug".*?(?=  - name:)/m, ""))

      error = assert_raises(IssueIntake::Invalid) { IssueIntake.validate!(root: root) }
      assert_includes error.message, "referenced label is absent"
    end
  end

  def test_form_cannot_auto_apply_priority_or_area
    with_fixture do |root|
      bug = File.join(root, ".github/ISSUE_TEMPLATE/bug.yml")
      File.write(bug, File.read(bug).sub('  - "status:triage"', "  - \"status:triage\"\n  - \"priority:p2\""))

      error = assert_raises(IssueIntake::Invalid) { IssueIntake.validate!(root: root) }
      assert_includes error.message, "default labels must be exactly"
    end
  end

  def test_blank_public_issues_and_missing_private_security_route_are_rejected
    with_fixture do |root|
      config = File.join(root, ".github/ISSUE_TEMPLATE/config.yml")
      File.write(config, "blank_issues_enabled: true\ncontact_links: []\n")

      error = assert_raises(IssueIntake::Invalid) { IssueIntake.validate!(root: root) }
      assert_includes error.message, "blank_issues_enabled must be false"
      assert_includes error.message, "private advisory"
    end
  end

  def test_catalog_label_missing_from_live_repository_is_rejected
    with_fixture do |root|
      labels = YAML.safe_load(
        File.read(File.join(root, ".github/labels.yml")),
        permitted_classes: [],
        permitted_symbols: [],
        aliases: false
      )
      names = labels.fetch("labels").map { |entry| entry.fetch("name") }
      missing = names.delete("environment:container")
      refute_nil missing
      live = File.join(root, "live-labels.txt")
      File.write(live, names.join("\n") + "\n")

      error = assert_raises(IssueIntake::Invalid) do
        IssueIntake.validate!(root: root, live_labels_path: live)
      end
      assert_includes error.message, "live repository is missing labels: environment:container"
    end
  end

  private

  def with_fixture
    Dir.mktmpdir("a2a-bridge-issue-intake-") do |root|
      FileUtils.mkdir_p(File.join(root, ".github"))
      FileUtils.cp_r(File.join(ROOT, ".github/ISSUE_TEMPLATE"), File.join(root, ".github"))
      FileUtils.cp(File.join(ROOT, ".github/labels.yml"), File.join(root, ".github/labels.yml"))
      yield root
    end
  end
end
