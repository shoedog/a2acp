# Roadmap issue stubs

Draft GitHub issues for the Horizon-1 items in [`../roadmap-improvements.md`](../roadmap-improvements.md).
These are **stubs**, not filed issues — GitHub Issues remains the canonical intake. File one with, e.g.:

```sh
gh issue create \
  --title "$(sed -n 's/^# //p' docs/roadmap-issues/h1-1-ttl-retention.md | head -1)" \
  --body-file docs/roadmap-issues/h1-1-ttl-retention.md \
  --label "kind:enhancement,priority:p1"
```

Once filed, link the issue number into [`../roadmap.md`](../roadmap.md) only when the item is actually
scheduled (per project convention).

| Stub | Item | Priority |
|---|---|---|
| [`h1-1-ttl-retention.md`](h1-1-ttl-retention.md) | M4 Slice 3b — TTL retention (bounded storage) | ★★★ |
| [`h1-2-cost-quota-governance.md`](h1-2-cost-quota-governance.md) | Cost & quota governance | ★★★ |
| [`h1-3-review-eval-expansion.md`](h1-3-review-eval-expansion.md) | Review-quality eval harness expansion | ★★★ |
| [`h1-4-serve-lifecycle.md`](h1-4-serve-lifecycle.md) | serve lifecycle & operator ergonomics | ★★ |
