package gosample

import "github.com/google/uuid"

// Add is a top-level function (workspace_symbol + definition target).
func Add(a, b int) int {
	return a + b
}

// Greeter is an interface (implementations target).
type Greeter interface {
	Greet() string // method `Greet` — surfaces via document_symbols children recursion
}

// En implements Greeter (the implementation `implementations(Greeter)` finds).
type En struct {
	Name string
}

func (e En) Greet() string { // method side of the duplicate `Greet`
	return "hi " + e.Name
}

// NewID uses the third-party uuid package — go-to-def at the `uuid.New` usage jumps into the module cache.
func NewID() string {
	return uuid.New().String()
}

// Double calls Add — gives call_hierarchy(Add, incoming) a real caller to find (else the test could
// always-empty-pass). Appended at the END so the line numbers the other tests assert stay stable.
func Double(x int) int {
	return Add(x, x)
}
