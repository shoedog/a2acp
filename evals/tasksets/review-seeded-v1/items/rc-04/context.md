# Collector (workflow node results)

`Collector` gathers one `NodeResult` per node of a workflow DAG. Each node gets
a cloned `Sender` and reports its single result; `collect()` drains them.

Contract:
- A DAG is loaded once and its node set is fixed and validated at load time
  (`nodes <= MAX_NODES = 64`). Each node sends exactly one result.
- Therefore the total number of messages is bounded by `nodes` -- the producer
  side is provably finite.

The change adds `Collector` using an unbounded `std::sync::mpsc` channel.
