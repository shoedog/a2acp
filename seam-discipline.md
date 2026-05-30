# Seam Discipline: Engineering Methodology for Long-Lived Systems

*Companion to `a2a-bridge-analysis.md` (v1) and `a2a-bridge-ecosystem.md` (v2)*
*Prepared for: Wesley Lambert — Senior Manager, Platform Engineering*
*Date: 2026-05-19*
*Status: Engineering Methodology Reference (v3)*

-----

## Table of Contents

- [1. Purpose and Scope](#1-purpose-and-scope)
- [2. Vocabulary: What a Seam Is](#2-vocabulary-what-a-seam-is)
  - [2.1 Feathers’ Definition](#21-feathers-definition)
  - [2.2 The Three Properties of a Seam](#22-the-three-properties-of-a-seam)
  - [2.3 Enforcement Across Languages](#23-enforcement-across-languages)
- [3. Core Seam-Creating Patterns](#3-core-seam-creating-patterns)
  - [3.1 Dependency Injection and Service Abstraction](#31-dependency-injection-and-service-abstraction)
  - [3.2 Hexagonal Architecture (Ports and Adapters)](#32-hexagonal-architecture-ports-and-adapters)
  - [3.3 Onion and Clean Architecture](#33-onion-and-clean-architecture)
  - [3.4 Anti-Corruption Layer](#34-anti-corruption-layer)
  - [3.5 Facade](#35-facade)
  - [3.6 Adapter](#36-adapter)
- [4. Patterns That Protect Seams From Erosion](#4-patterns-that-protect-seams-from-erosion)
  - [4.1 Tell, Don’t Ask](#41-tell-dont-ask)
  - [4.2 Law of Demeter](#42-law-of-demeter)
  - [4.3 Command-Query Separation](#43-command-query-separation)
  - [4.4 Return Interfaces, Not Concrete Types](#44-return-interfaces-not-concrete-types)
- [5. Patterns That Move Work to Compile Time](#5-patterns-that-move-work-to-compile-time)
  - [5.1 Newtypes and Type Wrapping](#51-newtypes-and-type-wrapping)
  - [5.2 Making Illegal States Unrepresentable](#52-making-illegal-states-unrepresentable)
  - [5.3 Phantom Types and Typestate](#53-phantom-types-and-typestate)
  - [5.4 Parse, Don’t Validate](#54-parse-dont-validate)
- [6. Patterns at the Protocol and Wire Boundary](#6-patterns-at-the-protocol-and-wire-boundary)
  - [6.1 Schema-First Contracts](#61-schema-first-contracts)
  - [6.2 Versioned Envelopes](#62-versioned-envelopes)
  - [6.3 Backwards-Compatible Message Evolution](#63-backwards-compatible-message-evolution)
  - [6.4 Tolerant Reader](#64-tolerant-reader)
- [7. Patterns at the Team and Process Level](#7-patterns-at-the-team-and-process-level)
  - [7.1 Architecture Decision Records](#71-architecture-decision-records)
  - [7.2 Conway’s Law as a Design Tool](#72-conways-law-as-a-design-tool)
  - [7.3 The Strangler Discipline](#73-the-strangler-discipline)
- [8. Rules of Thumb and Onboarding Exercise](#8-rules-of-thumb-and-onboarding-exercise)
  - [8.1 Heuristics for Detecting Seam Violations](#81-heuristics-for-detecting-seam-violations)
  - [8.2 An Onboarding Exercise](#82-an-onboarding-exercise)
- [9. Relationship to v1 and v2](#9-relationship-to-v1-and-v2)
- [Appendix A — Reference Reading](#appendix-a--reference-reading)

-----

## 1. Purpose and Scope

The first two documents in this series produced an analysis and a recommendation for a specific system, the A2A bridge. The arguments in those documents repeatedly turned on a single underlying property: whether the system’s component boundaries are mechanically enforced or merely conventional. That property was named the seam discipline, but it was treated as background to the recommendation rather than as a subject in its own right. This document promotes the seam discipline to a first-class topic and develops it as portable engineering methodology, applicable to Charter platform work well beyond the bridge.

The intended audience is engineers and architects who already practice dependency injection and service abstraction at the object level, and who want to understand why those practices generalize, what other patterns operate in the same family, and how language choice changes the strength of the resulting guarantees. The material draws on established engineering literature — Michael Feathers on legacy code, Alistair Cockburn on hexagonal architecture, Eric Evans on domain-driven design, Robert Martin on clean architecture, Martin Fowler on the strangler fig pattern — and synthesizes it with patterns more recently established in the Rust and typed-functional traditions. The synthesis is opinionated. Where the literature offers competing framings I have chosen the one I judge most useful for long-lived systems, and noted the alternatives where they would substantively change a reader’s choices.

The document is organized so that earlier sections establish vocabulary and the later sections apply it. Section 2 defines what a seam is and what enforcement means. Sections 3 through 7 cover five families of patterns: those that create seams, those that protect seams from erosion, those that move work to compile time, those that operate at protocol and wire boundaries, and those that operate at the team and process level. Section 8 distills practical heuristics and proposes an onboarding exercise for teams transitioning to languages with stronger seam enforcement. Section 9 places the document in relation to v1 and v2.

-----

## 2. Vocabulary: What a Seam Is

### 2.1 Feathers’ Definition

The term seam in this technical sense originates with Michael Feathers’ 2004 book *Working Effectively With Legacy Code*. Feathers defined a seam as a place in code where behavior can change without editing in that place. The canonical example is a function that calls a logger. If the logger is constructed inline within the function, there is no seam, because changing the logger requires editing the function. If the logger is passed as a parameter or accessed through an interface that can be swapped, there is a seam, because the logger’s behavior can change without the function’s code being touched.

Feathers introduced the term in the context of testability. The practical motivation for identifying seams was that legacy code without seams cannot be unit tested, because there is no way to substitute test doubles for production dependencies. The deeper insight, which has driven the term’s adoption beyond testing, is that the same property that makes code testable also makes it maintainable, swappable, and extensible. A seam is the architectural property that lets one part of a system evolve independently of the parts on the other side of it.

In the years since the book, the term has generalized from object-level boundaries to architectural-scale ones. Today a seam may sit between two methods on the same class, between two classes in the same module, between two modules in the same process, between two processes on the same host, or between two services across a network. The patterns and discipline that maintain seams are largely the same at every scale, which is why this material is portable across the range of systems Charter platform engineering builds.

### 2.2 The Three Properties of a Seam

A useful seam has three properties, and the absence of any one of them weakens or eliminates the others.

The first property is a stable contract. Each side of the seam must promise specific behavior to the other, expressed in a form both sides can agree on without ambiguity. The contract may take the form of an interface, a trait, an abstract base class, a wire protocol schema, or a documented API; what matters is that both sides reference the same artifact and that the artifact’s evolution is governed deliberately rather than incidentally. A seam without a stable contract is not a seam, it is wishful thinking dressed in an interface name.

The second property is a swap point. The contract must be invoked through some mechanism that admits substitution, whether at compile time, at startup, or at runtime. Constructor injection, factory methods, plug-in registries, and configuration-driven backend selection all qualify. A contract without a swap point is documentation, not architecture; it tells the reader how the components are intended to relate but does not force the running system to honor the intention.

The third property is an enforcement mechanism. Something must catch violations of the contract before they cause harm. Type checkers, schema validators, contract tests, fuzz tests, and runtime assertions are all valid enforcement mechanisms. Different mechanisms operate at different times, with very different cost profiles. A seam that is enforced only by convention is a seam that exists when the team is paying attention and erodes when the team is busy, which is to say, exactly when seam erosion is most dangerous.

These three properties are independent in principle but tend to vary together in practice. A team that takes one of them seriously tends to take all three seriously. A team that lets one of them lapse tends to let the others lapse as well. The reason is sociological rather than technical: seam discipline is a habit, not a policy, and the team’s habits are coherent across the patterns that express them.

### 2.3 Enforcement Across Languages

The enforcement property is where languages differ in ways that translate directly into long-term maintenance cost. Every modern general-purpose language can express seams. The languages differ in how aggressively they enforce them, and the difference is mechanical rather than stylistic.

Rust enforces seams at compile time through its trait system and ownership rules. A function that accepts a trait object or a generic parameter constrained by a trait can call only methods defined on that trait, and the compiler refuses to compile code that reaches around the trait into the implementation. The ownership and borrowing rules additionally enforce that only one writer accesses mutable state at a time, which closes off the class of seam violations that show up as data races in less-strict languages. The result is that the seam contract is mechanically inviolable: the team can be careless, distracted, or new to the project, and the compiler will catch the violation before it reaches code review.

Go enforces seams at compile time through interfaces, but with two significant weakenings relative to Rust. First, Go interfaces are structural rather than nominal, meaning that any type with the right method signatures satisfies the interface, even unintentionally. This makes some refactors easier but means that “I implement this seam” is implicit rather than declared, and a developer can satisfy an interface by accident or stop satisfying it without notice. Second, Go does not enforce concurrency rules across the seam, so two goroutines can call the same client concurrently with no compile-time warning. The race detector under load testing catches a subset of the resulting bugs, but the cost of detection is higher and the latency from defect introduction to defect discovery is longer than in Rust.

TypeScript enforces seams at write time and at `tsc` time, but the enforcement is erased before the program runs. Any JSON parse, any value crossing the language boundary into native or wasm code, and any explicit type assertion bypasses the type system entirely. The seam exists in the type annotations; whether it exists in the running program depends on whether everyone remembered to validate at the boundaries. Zod and similar runtime validation libraries can close some of the gap, but they are a discipline rather than a guarantee, and discipline degrades under deadline pressure in ways that compiler enforcement does not.

Python’s enforcement is the weakest of the four. Type hints document seams; mypy or Pyright check them; nothing requires the team to run those tools. Pydantic provides runtime validation comparable to Zod. The seam exists if the team chooses to maintain it; the language does not assist. For short-lived scripts and exploratory work this is appropriate, since the cost of strong enforcement exceeds the benefit at small scale. For long-lived production services it is the most expensive choice, because every team-membership change, every dependency upgrade, and every refactor is an opportunity for an unchecked seam to erode unnoticed.

The practical implication is that language choice determines the steady-state strength of the seam discipline a team can sustain. A team with strong seam habits will produce better systems in any language; a team without strong seam habits will produce more durable systems in Rust than in Python, regardless of intent. Charter platform engineering’s existing practices around dependency injection and service abstraction are a foundation, and the language layer either reinforces or undercuts that foundation depending on which language is chosen for which system.

-----

## 3. Core Seam-Creating Patterns

The patterns in this section are the architectural moves that establish seams in the first place. They differ in scale and in the kind of boundary they create, but they share the common purpose of separating concerns across an enforceable line.

### 3.1 Dependency Injection and Service Abstraction

Dependency injection is the technique by which a component receives its dependencies from outside rather than constructing them internally. Service abstraction is the related practice of accessing those dependencies through interfaces rather than concrete types. Together they are the foundation of seam discipline at the object level, and the pattern most engineering teams adopt first because the testability benefit is immediately visible.

The mechanics are well understood. Instead of a class that constructs its own database connection in a constructor, the class accepts a connection as a parameter. Instead of accepting a specific connection class, the class accepts any value implementing a `Connection` interface. The interface is the seam contract, the constructor parameter is the swap point, and the type system or the testing framework provides the enforcement mechanism. The class can be tested with a fake connection, deployed against a real database, and migrated to a different database technology, all without internal changes.

What is worth being explicit about is that dependency injection is not a single pattern but a family of techniques that vary in how strongly they support the three seam properties. Constructor injection is the strongest variant because the dependency is declared in the type signature and cannot be omitted or substituted later. Setter injection is weaker because the dependency can be absent at construction time, requiring runtime checks. Service locator patterns are weaker still because the dependency is looked up by name at the point of use, and the lookup table is mutable state. Framework-driven injection containers, such as Spring in Java or NestJS in TypeScript, range across this spectrum depending on configuration, and tend to introduce framework-specific failure modes (cyclic dependencies discovered at startup, missing bindings discovered in production) that constructor injection avoids by construction.

Rust’s idiomatic dependency injection is the strictest variant of constructor injection. There is no container, no annotation-driven autowiring, and no service locator. Dependencies are passed as constructor parameters typed against traits, either as trait objects through `Arc<dyn Trait>` or as generic parameters through `impl Trait`. The lack of a framework feels primitive to engineers arriving from richer DI ecosystems, but the simplicity is itself a feature for long-lived systems, because there is no framework upgrade cycle to manage and no magic behavior to debug. The wiring is regular code, traceable in the editor, and verified by the compiler at every build.

The teams that get the most value from DI and service abstraction are the teams that recognize the practice as architecture rather than testing accommodation. Mocking for tests and swapping for production deployment are the same operation; the seam does not distinguish between them. Once a team internalizes this, the discipline begins to pay across the lifecycle: the same seam that supported the original test suite supports the migration to a new backend three years later, with the same low cost.

### 3.2 Hexagonal Architecture (Ports and Adapters)

Hexagonal architecture, introduced by Alistair Cockburn in 2005, generalizes the seam discipline from the object level to the application level. The architecture’s organizing principle is that the application core defines ports describing what it needs from the outside world, and adapters implement those ports against specific external technologies. The core knows nothing about HTTP, SQL, JSON serialization, message brokers, or specific wire protocols; it knows only that it has a `UserRepository`, an `OrderPublisher`, an `EmailGateway`. The ports are the seams, the adapters are the implementations, and the core is testable in isolation because every external dependency can be substituted with a test double.

The visual metaphor of a hexagon, with the core at the center and adapters around the perimeter, is incidental to the substance of the pattern. The substance is the architectural commitment that the core has no outbound dependencies on infrastructure technologies, only on the port interfaces it defines. This commitment is harder to maintain than it sounds, because the temptation to leak infrastructure concepts into the core is constant. A `UserRepository` port that returns `SQLResultSet` has already failed the architecture, even if the type is wrapped in a domain class. A port that takes `HttpRequest` as an argument has failed it from the other direction. The core’s vocabulary must be entirely domain-shaped.

The benefit of the architectural commitment is that the core becomes durable across infrastructure changes. The same business logic that ran against an in-memory store during development runs against a Postgres database in production, then migrates to a sharded database five years later, all without core changes. The core can be tested without standing up databases, message brokers, or HTTP servers, which makes the test suite fast and the test failures specific. The core can be reused across deployment topologies, including ones the original designers did not anticipate.

The cost of the architecture is upfront design effort and a multiplication of types. A simple application that would consist of one class with database calls in its methods becomes a core with a port, an adapter implementing the port, and a wiring layer connecting them. For small applications this is over-engineering. For applications expected to live more than two or three years, and to face changes in their infrastructure dependencies, the cost is repaid many times over.

For the bridge specifically, the hexagonal shape is the natural fit. The bridge’s core is the translation logic between A2A task semantics and ACP session semantics. The ports are the inbound A2A protocol, the outbound ACP protocol, the session store, the policy engine, and the observability surface. Each port has at least one adapter, and most will eventually have several. The `tomtom215/a2a-rust` crate’s claim to follow hexagonal architecture principles is a substantive promise about its testability and evolution, not a marketing phrase, and the bridge’s own architecture should preserve the same shape.

### 3.3 Onion and Clean Architecture

Onion architecture, articulated by Jeffrey Palermo in 2008, and clean architecture, articulated by Robert Martin in 2012, are repackagings of hexagonal architecture with somewhat more prescriptive layering. The core sits at the center, with successive concentric layers wrapping it: domain entities, then use cases, then interface adapters, then framework and driver code. Dependencies point inward only, meaning that no inner layer references an outer layer’s types. This is the dependency inversion principle made architectural.

In practice, the differences between hexagonal, onion, and clean architecture are largely vocabulary. All three express the same commitment: the application’s core depends on abstractions, infrastructure implements those abstractions, and the direction of dependency runs from outside to inside. Teams adopting any one of them get most of the same benefits. The choice between them is often a question of which book the team has read, and the larger risk is that the team adopts the vocabulary without the discipline, producing layered diagrams in documents while writing code that ignores the layer boundaries.

The single substantive caveat worth raising about clean architecture specifically is that its prescriptive layering can encourage a multiplication of types beyond what the problem requires. A simple CRUD operation can become a controller, a request DTO, a use case, a domain entity, a repository interface, a repository implementation, a database row type, and several mappers between them. Each layer is justifiable in principle; the aggregate is sometimes ceremonial. The hexagonal framing is generally less prescriptive about how many layers must exist and tends to produce leaner systems in my experience, but reasonable engineers disagree on this and the choice should be made deliberately rather than by default.

### 3.4 Anti-Corruption Layer

The anti-corruption layer, introduced by Eric Evans in *Domain-Driven Design* (2003), is the pattern of placing a translation layer between two systems whose models differ, so that the semantics of one system do not contaminate the other. The pattern arose in the context of integrating with legacy systems whose data models reflected obsolete or vendor-specific concepts that would degrade the integrating system if imported directly. The translation layer converts incoming data into the integrating system’s preferred model and converts outgoing data back, isolating the integrating system from the upstream’s quirks.

The pattern generalizes well beyond legacy integration. Whenever two systems with distinct models communicate, and the cost of conforming either model to the other is unacceptable, an anti-corruption layer between them is the appropriate response. The bridge in v1 is exactly this: A2A’s task lifecycle and ACP’s session lifecycle are semantically distinct, and the v1 document’s observation that the mapping is “not injective” is the technical statement that neither model is a subset of the other. The bridge translates between them, and the discipline the bridge must maintain is that neither protocol’s vocabulary leaks into the other side.

The practical work of an anti-corruption layer is harder than it appears. The translation logic must handle not only the cases where both sides have analogous concepts, but also the cases where one side has a concept the other does not. An A2A `tasks/cancel` request must translate to an ACP `session/cancel` if the session is active, to a no-op if the session has already terminated, and to a specific error response if the session never existed. An ACP `session/update` notification with a permission request must translate to an A2A `input-required` task state, even though A2A has no native concept of fine-grained permission flows. These mappings are where the bridge’s value lives, and where naive implementations fail.

The pattern’s name is occasionally misread as adversarial, as though the upstream system is the corrupter and the integrating system the victim. The intent is closer to architectural hygiene: each system maintains its own coherent vocabulary because each system reasons better internally when its model is consistent, and the layer between them absorbs the mismatch.

### 3.5 Facade

The facade pattern, from the Gang of Four design patterns catalog, provides a simplified interface to a complex subsystem. The facade exposes the operations a particular client needs in the form the client finds convenient, and hides the subsystem’s internal structure. The facade is itself a seam, in that the client depends on the facade rather than on the subsystem’s internals, and the subsystem can be reorganized without the client noticing.

The facade differs from the bridge in a subtle but important way. A bridge translates between two protocols that both have first-class status; neither is subordinate to the other, and the bridge mediates their interaction. A facade simplifies a single subsystem for a single client’s benefit; the subsystem’s full surface remains available to other clients, and the facade is an additional convenience rather than an exclusive boundary.

In the bridge architecture, the per-agent adapters are facades. The Claude Code adapter exposes a uniform `AgentClient` interface upward and hides the messy reality of how Claude Code is actually driven (through the Zed adapter, through the Python `claude-code-acp` package, or through a future native Rust reimplementation). The Kiro adapter does the same for Kiro CLI. The adapters are facades because they exist to simplify, not to translate; the underlying ACP protocol is the same on both sides of the facade, and the facade exists to absorb the differences in how each specific CLI implements the protocol.

The reason it is worth distinguishing facade from bridge precisely is that the two patterns have different evolution characteristics. A bridge is a stable boundary between stable protocols; its job ends when both protocols stabilize. A facade is a convenience layer over an evolving subsystem; its job continues for as long as the subsystem evolves, which in the case of CLI agents is indefinitely. Designing one when you need the other produces friction.

### 3.6 Adapter

The adapter pattern, also from the Gang of Four catalog, converts the interface of an existing class into the interface a client expects. Where the facade provides a simpler interface, the adapter provides a different interface; the difference is intent, not mechanism. The adapter pattern is what makes incompatible components compatible without modifying either of them.

In the agent ecosystem, the existing ACP harnesses are adapters in the strict pattern sense. `cola-io/codex-acp` adapts the Codex CLI’s native interaction model to the ACP interface. The Zed `claude-agent-acp` adapter does the same for Claude Code. Each adapter exists because the underlying tool was built before ACP standardized and cannot be modified to implement ACP natively, so the adapter sits in between and presents an ACP-compliant interface to clients while driving the tool through its actual interface.

The adapter is the conceptual ancestor of the harness shape from v2. A harness extends an adapter with process supervision, permission policy, and isolation, but the core mechanic is the same: present a protocol-compliant interface upward, and absorb the implementation details of the wrapped tool downward. The recognition that harnesses are adapters at heart is useful because the adapter pattern’s known weaknesses then apply: tight coupling to the wrapped tool’s specifics, brittleness under the wrapped tool’s evolution, and a per-tool engineering cost that cannot be amortized across tools. These weaknesses are visible in the ACP harness ecosystem and explain why the harness category requires continuous maintenance attention.

-----

## 4. Patterns That Protect Seams From Erosion

Creating a seam is necessary but not sufficient. Seams are subject to a slow erosion that occurs every time a developer takes a shortcut around the boundary they were supposed to respect. The patterns in this section are the discipline that keeps seams intact under the steady pressure of day-to-day development.

### 4.1 Tell, Don’t Ask

The Tell-Don’t-Ask principle, formulated by Andy Hunt and Dave Thomas, says that an object should be asked to do something rather than queried for its state so the caller can decide what to do. The contrast is concrete. The asking style fetches the account balance, compares it to the withdrawal amount in caller code, and then writes back a new balance. The telling style invokes `account.withdraw(amount)` and lets the account itself enforce the invariants.

The principle sounds trivial until one observes how often the asking style appears in code that uses dependency injection and interfaces. The interface declares getter methods that return internal state; callers fetch the state, reason over it, and call setter methods to update it. The seam exists in the type signature but is hollow in practice, because the callers have effectively pulled the implementation’s logic up into themselves. When the implementation needs to change its internal representation, the callers all break, and the seam was a polite fiction.

The corrective discipline is to design interfaces in terms of operations rather than state. An interface that exposes `withdraw`, `deposit`, `transfer_to` is honoring the seam. An interface that exposes `get_balance`, `set_balance`, `get_transaction_log`, `append_transaction` is leaking. The discipline costs nothing at write time and pays continuously at change time, because operations are stable units of meaning while state representations are not.

The principle generalizes from objects to architectural boundaries. A service interface that exposes operations meaningful in the domain language is robust. A service interface that exposes CRUD operations on internal data structures is the same anti-pattern at a larger scale. Teams that internalize Tell-Don’t-Ask at the object level transfer the discipline upward naturally; teams that do not, accumulate CRUD-shaped service interfaces and discover years later that their architecture is a thin wrapper over their database schema.

### 4.2 Law of Demeter

The Law of Demeter, formulated at Northeastern University in the 1980s, is the principle of least knowledge. A method should call only methods on its own fields, its parameters, objects it creates, and its direct dependencies, never on objects returned from those calls. The phrase “talk to friends, not strangers” captures the intent.

The violation pattern is the chained method call. An expression like `session.get_agent().get_config().get_model()` reaches three levels into the object graph, coupling the caller to the existence and shape of every intermediate type. If the agent’s configuration is restructured, or the model lookup moves elsewhere, the caller breaks even though it nominally depends only on the session. The seam between caller and session was crossed three times, and each crossing was a fresh coupling.

The corrective discipline is to add operations to the directly-depended-on type that hide the intermediate structure. Instead of asking the session for the agent, the agent for the config, and the config for the model, the caller asks the session for the model directly. The session’s implementation can chain internally if it wishes, but the caller does not see the chain. The Law of Demeter is sometimes derided as the “law of one dot” and treated as overzealous, but the underlying property it protects is real: the depth of the object graph a caller penetrates is a measure of the coupling the caller has accumulated, and accumulated coupling is what causes diffuse breakage under refactor.

The principle has well-known exceptions. Fluent interfaces and builder patterns chain method calls deliberately, returning self-similar types whose chain depth does not represent coupling. Method chains on standard library types like collections and iterators are not Demeter violations because the type is part of the language’s stable vocabulary. The principle is a heuristic for noticing when chained calls indicate accidental coupling, not a syntactic prohibition.

### 4.3 Command-Query Separation

Command-Query Separation, formulated by Bertrand Meyer, says that every method should be either a command, which changes state and returns no value, or a query, which returns information and has no side effects, but never both. The separation is partly aesthetic and partly architectural.

The architectural benefit is that queries become safe to call repeatedly, safe to cache, safe to call from any thread, and safe to use as predicates in conditional logic, while commands are clearly identified as the operations that require careful sequencing. A seam interface designed around the separation is more reasonable to consume: a developer reading the interface knows immediately which methods are observation and which are action.

The discipline is harder to maintain than it sounds because there is constant pressure to write methods that both modify state and return information about the modification. The C-style increment operators that both return and modify, the database operations that return generated keys, the queue operations that pop-and-return, are all violations rationalized by convenience. Sometimes the convenience is worth the cost; usually it is not. The cleaner separation is to have a command that performs the action and a separate query that returns the relevant subsequent state, possibly by accepting a callback or returning an event that downstream consumers can subscribe to.

For protocol bridges specifically, command-query separation is particularly valuable because retry semantics depend on it. A query can be retried freely. A command must be idempotent or guarded by an idempotency key to be safely retried. A method that does both forces every caller to reason about both concerns simultaneously, and most callers will get one of them wrong. Keeping the two separate at the interface level makes the retry logic a property of the interface, not a property of every caller’s discipline.

### 4.4 Return Interfaces, Not Concrete Types

The smallest and one of the most frequently violated seam-protecting disciplines is to declare return types as interfaces rather than as concrete types. A method that returns `SqlSession` has already leaked SQL across the seam, regardless of whether the caller uses any SQL-specific functionality. A method that returns `Session` (an interface that `SqlSession` implements) preserves the seam, because the caller cannot reach into SQL-specific behavior without an explicit downcast that the team’s code-review discipline can catch.

The violation usually happens by accident. A method is written returning the concrete type because it is convenient to do so at write time, and no caller initially uses any concrete-specific functionality. Over time, callers discover concrete-specific functionality and use it. The discovery propagates the leak without anyone noticing, because each individual caller’s use of the concrete type seems innocuous. By the time the team wants to swap implementations, the leak is pervasive and the swap is impossible without extensive refactoring.

The corrective discipline is to declare return types defensively, defaulting to the narrowest interface that satisfies the caller’s actual needs. This sometimes requires defining new interfaces to capture the narrow needs, which feels like overhead at write time and pays continuously at change time. Rust’s `impl Trait` return types make this discipline cheap to apply, since the concrete type need not be named at the call site. Other languages require more explicit interface declaration but the principle is the same: the return type is a contract, and the narrower the contract, the more durable the seam.

-----

## 5. Patterns That Move Work to Compile Time

The patterns in this section are the ones where typed languages, and Rust in particular, earn their weight. They are techniques for representing domain constraints in the type system such that violations of those constraints become compile errors. Each pattern eliminates a category of test cases by making the underlying bug impossible to write.

### 5.1 Newtypes and Type Wrapping

The newtype pattern wraps a primitive type in a domain-specific type so that the type system can distinguish between values that share a representation but mean different things. A `SessionId(String)` and a `CallerId(String)` are both strings at runtime, but the type system refuses to let one be passed where the other is expected. A function that accepts a `SessionId` cannot accidentally be called with a `CallerId`, even though both are strings, because the wrapping type is different.

The pattern’s value is most visible in functions with multiple parameters of the same primitive type. A function that takes two strings, where one is a session ID and the other is a caller ID, is a bug factory; sooner or later the arguments will be passed in the wrong order, and the resulting failure may be silent. The same function rewritten to take a `SessionId` and a `CallerId` is mechanically protected against the swap, because the compiler refuses to type-check the call with arguments in the wrong order.

Rust’s newtype is a struct wrapping a primitive, with no runtime cost in the common case. TypeScript’s branded types achieve the same effect at write time and at `tsc` time through intersection with a tag type. Go has named types that distinguish at compile time, though without the methods that newtypes typically carry in other languages. Python’s NewType from the typing module documents the intent for mypy and Pyright but is erased at runtime.

The discipline of using newtypes liberally is one of the cheapest seam-protection techniques available. Identifiers, timestamps, currencies, units of measure, validated email addresses, and any other primitive that means something specific are all newtype candidates. The cost is a small amount of declaration boilerplate; the benefit is the elimination of entire classes of argument-confusion bugs.

### 5.2 Making Illegal States Unrepresentable

The principle of making illegal states unrepresentable, popularized in the OCaml and F# communities and now widely adopted in Rust, is that the type system should refuse to represent states that should not exist in the program. Instead of a `Session` struct with a `is_active` boolean and a nullable `session_id`, the principle calls for a `Session` enum with `Inactive`, `Initializing`, and `Active { session_id }` variants. The state “inactive but has a session ID” cannot be represented, because no variant of the enum allows it.

The transformation has cascading effects on the rest of the code. Code that operates on a `Session` must handle every variant, because the compiler refuses to compile incomplete `match` expressions. The questions that a struct-and-flags representation raises at every use site — “is this session active? does it have an ID? what should I do if it has an ID but isn’t active?” — disappear, because the impossible combinations no longer exist as values to reason about.

Rust’s algebraic data types make the pattern idiomatic and efficient. The compiler optimizes enum representations down to the size of their largest variant, and exhaustive match is enforced by default. TypeScript’s discriminated unions provide a similar mechanism with similar enforcement, though with more verbose declaration. Go does not have native sum types and emulates them through interfaces and type switches, with substantially weaker enforcement; the compiler will not warn about missed cases. Python’s enum module and the structural pattern matching introduced in 3.10 approach the capability but require explicit attention to maintain.

The discipline pays disproportionately for protocol implementations and state machines, both of which are explicit subjects in the bridge architecture. A bridge has a session lifecycle, a task lifecycle, a permission state machine, and a protocol-version negotiation, all of which are state machines whose illegal-state transitions are the source of most bugs. Encoding each as an enum with state-specific data eliminates a significant fraction of the test cases that would otherwise be necessary.

### 5.3 Phantom Types and Typestate

The typestate pattern extends the principle of unrepresentable illegal states by encoding state machines into the types themselves, rather than into the values. A `Session<Initializing>` and a `Session<Active>` are different types, parameterized by a phantom type that exists only at compile time. Methods are defined on specific instantiations: `initialize()` is defined on `Session<Initializing>` and returns a `Session<Active>`, while `send_prompt()` is defined only on `Session<Active>`. The compiler refuses to call `send_prompt()` on a session that has not yet been initialized, because the type does not have that method.

The pattern is more invasive than enum-based state representation, because every operation that handles a session must be parameterized over its state or restricted to a specific state. The benefit, when the cost is justified, is that the entire class of “operation called in the wrong state” bugs disappears. Protocol implementations are the canonical motivating example, because protocols typically have strict ordering requirements that the typestate pattern enforces mechanically.

Rust supports the pattern idiomatically through generic type parameters with phantom data. TypeScript can express it through conditional types and template literal types with substantial complexity. Go cannot meaningfully support it. The pattern is overkill for many situations, particularly those where the state machine has few states or where the ordering requirements are loose. It is exactly right for situations where the protocol’s correctness depends on ordering, which is the situation that ACP and A2A both create.

The honest tradeoff is that typestate code is harder to read for engineers who have not encountered the pattern before. Onboarding cost is real, and the benefit is invisible to engineers who have not yet experienced the bugs the pattern prevents. The recommendation is to apply typestate selectively, at the boundaries where protocol correctness matters most, and to document the pattern clearly when it is used so that future maintainers can read the code as type-system-enforced documentation rather than as obscure type acrobatics.

### 5.4 Parse, Don’t Validate

The Parse-Don’t-Validate principle, articulated by Alexis King in 2019, says that data crossing a wire boundary should be transformed into a domain type whose existence implies validity, rather than checked for validity and passed through as raw structure. The contrast is between a function that takes a string and validates it as an email address (returning a boolean) versus a function that takes a string and returns an `EmailAddress` or an error (the `EmailAddress` type having no other constructor).

After parsing, downstream code receives the `EmailAddress` type and can rely on its validity without re-checking. After validation, downstream code receives the original string and must either re-validate or trust the caller’s claim of prior validation, both of which are forms of erosion. The parse style pushes validation to the boundary and makes the boundary explicit; the validate style scatters validation throughout the program and erodes the boundary’s significance over time.

The pattern combines naturally with newtypes. A `ValidatedSessionId` newtype with no public constructor, only a fallible `parse()` method, captures both ideas at once. The newtype is the seam between trusted and untrusted data; parsing is the only legitimate way to cross from untrusted to trusted; downstream code receiving a `ValidatedSessionId` can rely on its validity by construction.

For protocol bridges, the principle has specific operational implications. Every inbound message arrives as untrusted data and must be parsed into the bridge’s internal model. Every outbound message must be constructed from the internal model and serialized for the wire. The parse boundary is the bridge’s actual trust boundary, and the discipline is to perform all validation at parse time, produce strongly-typed internal values, and let downstream code operate on those values without re-validating. The internal model is more strongly typed than the wire format; this is the architecture, not an inefficiency.

-----

## 6. Patterns at the Protocol and Wire Boundary

The patterns in this section govern the boundary where the system meets external counterparts. They are the discipline of protocol evolution and inter-system compatibility.

### 6.1 Schema-First Contracts

The schema-first discipline is to define the wire protocol in a schema artifact — JSON Schema, Protobuf, OpenAPI, GraphQL SDL — and to generate types on both sides of the boundary from that schema, rather than hand-writing types that nominally conform to the protocol. The schema is the seam contract; both sides drift independently of each other but conform to the same source of truth.

The mechanical benefits are immediate. When the schema changes, both sides regenerate types and incompatibilities surface as compile errors rather than as runtime parsing failures. When a new field is added to the schema, both sides receive it consistently without manual coordination. When the schema is versioned, both sides can support multiple versions in parallel without diverging interpretations.

The discipline is to never hand-write a wire type. Hand-written types drift from the schema, develop subtle interpretation differences across implementations, and become a source of integration debugging that the generated approach avoids entirely. ACP’s official approach demonstrates the pattern: there is a JSON Schema for the protocol, the Rust SDK uses generated types, the Python SDK uses generated Pydantic models tracking the same schema, and version compatibility is verified mechanically. The bridge in v1 inherits this discipline automatically by depending on the official SDKs rather than implementing wire types independently.

The investment cost is the schema infrastructure: code generators, build integration, schema version management. For one-off integrations the cost exceeds the benefit, and hand-written types are appropriate. For long-lived systems with externally-defined protocols, the discipline is the difference between a system that ages well and one that becomes brittle as the protocol evolves.

### 6.2 Versioned Envelopes

The versioned-envelope pattern is to include an explicit protocol version field in every wire message. The seam handler reads the version at the boundary, dispatches to the appropriate parser, and produces a canonical internal representation. The discipline that goes with the pattern is to never branch on version inside business logic — branch at the parse boundary and let downstream code reason in version-independent terms.

The pattern’s purpose is to admit multiple protocol versions in parallel during transition periods. Without explicit versioning, a system must commit to a single protocol version at any time, and protocol upgrades require coordinated cut-overs. With explicit versioning, old and new clients can interact with the same server, and the server can drop support for old versions on its own schedule rather than the clients’.

ACP implements this pattern through the `protocolVersion` field exchanged during the `initialize` handshake. A2A implements it through agent card declarations and request headers. The bridge inherits both versioning schemes and must surface mismatches as clear errors rather than as ambiguous behavior. The bridge’s own internal data model should be the canonical representation that all wire versions translate into; the wire versions exist at the edges, not in the middle.

The pattern’s failure mode is allowing version branching to creep inward from the parse boundary. Once business logic contains `if version == 1 { ... } else { ... }` branches, the code’s complexity scales multiplicatively with the number of versions supported. The discipline to keep version branching at the parse boundary is essential and not always intuitive, because the easiest place to put a version branch is wherever the version difference is first felt, which is usually downstream of the parse boundary.

### 6.3 Backwards-Compatible Message Evolution

The discipline of evolving wire protocols without breaking existing implementations is a body of practice in itself. The core rules are to add fields rather than removing them, to make new fields optional with defaults that preserve old behavior, to never reuse field names or numbers for different purposes, and to use enums with explicit `Unknown` or `Unrecognized` variants so that forward-compatible values can be tolerated.

Protobuf bakes most of these rules into the protocol itself. JSON Schema allows them but does not enforce them. Ad-hoc JSON shapes invite their violation, particularly when types are hand-written rather than generated. The discipline applies regardless of the underlying technology; the technology determines how much help the team gets in maintaining the discipline.

For the bridge, the practical consequence is that the internal data model should be liberal in what it accepts from the wire, conservative in what it produces, and tolerant of fields it does not recognize. A response from a future ACP version that includes fields the bridge does not understand should be parsed successfully, with the unknown fields ignored, not rejected as malformed. This is the tolerant-reader principle described next.

### 6.4 Tolerant Reader

The tolerant-reader principle, attributed to Martin Fowler, is to be conservative in what you send and liberal in what you accept. Parse and use only the fields you know about; ignore unknown fields rather than failing on them. The principle’s name borrows from Jon Postel’s robustness principle for networking protocols, expressing the same idea at the application layer.

The pattern’s value is durability across protocol drift. When the upstream system adds a new field, downstream tolerant readers continue to function without modification. When the upstream system reorganizes its responses, downstream tolerant readers continue as long as the fields they actually use remain. The cost is some debugging clarity, because a typographical error in a field name produces silent ignoring rather than an explicit error, and the diagnostic loop is longer.

The pattern is generally the right tradeoff for protocol bridges. The bridge does not own the protocols it speaks; both A2A and ACP evolve outside the bridge’s control. Being tolerant of the protocols’ evolution preserves the bridge’s value as the protocols change, and the alternative — strict rejection of any unexpected field — would require version-locked deployment coordination with every protocol upgrade.

The discipline has a counterpart on the production side. Being conservative in what you send means producing only fields whose interpretation the recipient can be expected to share, defaulting to the minimum subset of the protocol that satisfies the operation. The combination of liberal acceptance and conservative production is the protocol-engineering posture that survives the longest across version drift, and it is the posture the bridge should adopt explicitly.

-----

## 7. Patterns at the Team and Process Level

The patterns in this section operate above the code itself, at the level of how the team makes and records decisions. They are the discipline that keeps the technical patterns alive across team membership changes and across the years during which a system is maintained.

### 7.1 Architecture Decision Records

An Architecture Decision Record is a short document, typically a single markdown file, that captures the context and reasoning for a specific architectural decision. The format, popularized by Michael Nygard, is structured: title, status, context, decision, consequences. The format’s economy is part of its value; ADRs are intended to be cheap to write and quick to read, not comprehensive whitepapers.

The discipline’s purpose is to preserve the reasoning behind decisions across the time during which the people who made them leave or forget. A team that uses ADRs can ask “why is this seam here?” and find an answer three years later. A team without them cannot, and the absence of recoverable reasoning leads to two failure modes: the team preserves seams whose original justification has expired, treating them as immutable because nobody remembers why they exist, or the team removes seams whose original justification is still valid, because the original reasoning has been lost.

The lighter-weight forms of the discipline are sometimes sufficient. A commit message that explains the why rather than the what, a comment in code citing the specific tradeoff considered, a wiki page summarizing the decision — all are valid carriers of the reasoning. The format matters less than the habit. The substantive practice is that decisions affecting the seam structure are documented at the time they are made, with enough context that the reasoning can be reconstructed by someone not present at the original conversation.

The discipline is one of the few engineering practices that has compounding returns over time. Each ADR is a small investment; the body of ADRs accumulates into a corpus that supports onboarding, supports re-evaluation of past decisions when circumstances change, and supports the architectural conversations that the team will inevitably have when the system’s scope shifts. Charter platform engineering’s longer-lived projects are exactly the contexts in which the compounding pays.

### 7.2 Conway’s Law as a Design Tool

Conway’s Law, formulated by Mel Conway in 1968, observes that systems mirror the communication structure of the organizations that build them. A team that communicates as a single unit produces a single integrated system. Three teams that communicate primarily within themselves produce three integrated systems with thin connections between them. The shape of the software follows the shape of the team.

The observation began as descriptive but is widely used today as prescriptive. If a team wants a particular system structure, it should organize the team to match. The inverse Conway maneuver — restructuring the team to produce the desired system architecture — is a recognized technique in platform engineering practice, particularly in organizations adopting microservices or domain-driven architectures.

For seam discipline specifically, Conway’s Law has a sharp implication. Seams between components erode when a single team owns both sides of the seam, because the team’s internal communication crosses the seam freely and the seam becomes a polite fiction. Seams between components are reinforced when distinct teams own each side, because cross-team communication is expensive enough to keep the seam meaningful. The implication is that a multi-layer agent stack will keep its layers cleanly separated only if the layers have distinct owners; under single-team ownership, the layers will tend to merge.

For the bridge specifically, the immediate practical consequence is that the bridge should be a single coherent component with a single owner during v1, and the layered extraction described in v2 should be triggered by the addition of teams as much as by the addition of consumers. Charter’s adoption of the bridge, if it occurs, is the inflection point at which Conway’s Law begins to apply, and the architecture should be ready to receive the resulting team-shaped pressure.

### 7.3 The Strangler Discipline

The strangler fig pattern, articulated by Martin Fowler in 2004, describes the gradual replacement of a legacy system by routing more and more traffic through a new system that wraps and forwards to the legacy. The image is botanical: a strangler fig vine grows around a host tree, slowly replacing it, until the original tree is gone and the vine remains, having taken its shape. The pattern’s appeal is risk control; each migration step is small, each is reversible, and the cumulative effect is large.

You already know this pattern well; it is the playbook for Project Vulcan’s migration of the Node.js monolith to Python FastAPI and Go microservices. The reason it appears here is that the strangler discipline includes a seam discipline that is sometimes underemphasized. The facade between the new system and the legacy system is itself a seam, often the most important one in the migration. The discipline of not leaking legacy concepts through the facade is what keeps the new system architecturally coherent as the migration proceeds. The same discipline that the bridge requires between A2A and ACP applies to every strangler fig the team will ever build.

The failure mode of strangler migrations is to leave the facade in place forever, with bits of the legacy system embedded behind it, because the cost of finishing the migration exceeds the perceived benefit of cleanup. The bits remain, the facade calcifies around them, and the system inherits a permanent layer of historical artifact. The discipline to drive strangler migrations to completion, removing the facade when its job is done, is the same discipline that keeps the bridge from accumulating cruft when one of its supported CLI agents reaches end-of-life or is replaced.

-----

## 8. Rules of Thumb and Onboarding Exercise

This section distills the preceding material into operational heuristics and proposes a concrete exercise for transitioning a team into stronger seam discipline.

### 8.1 Heuristics for Detecting Seam Violations

The patterns in sections 3 through 7 give the framework, but day-to-day engineering rarely operates at framework level. The following heuristics are useful in code review and in pair programming, where the question is not “is this the right architecture” but “is this specific change eroding something important.”

When a developer writes an `instanceof` check or an explicit downcast, a seam is leaking. The downcast indicates that the interface declared at the seam is not actually sufficient for what the caller wants to do, which means either the interface needs to grow or the caller needs to rethink its dependency. Either response preserves the seam; the downcast itself does not.

When a developer wants to “just peek” at an implementation’s internal state from outside the interface, the abstraction is wrong. The peek is an asking move in a context that calls for telling. The corrective is either to add an operation to the interface that captures what the caller actually wants to accomplish, or to recognize that the caller’s dependency on internal state is itself the bug.

When tests require monkey-patching, runtime patching, or framework-specific mocking decorators to function, the seam is in the wrong place. Real seams support mocks through their declared types; if the mock requires bypassing the type system, the type system was not actually carrying the contract. The corrective is to refactor the production code until the test can supply a substitute through the normal type-system mechanism.

When code branches on protocol version, transport type, or backend implementation in business logic, the branch belongs at a seam rather than in the business logic. The corrective is to push the branching to the edge and produce a canonical internal representation that downstream code can operate on without re-branching.

When two layers want to share a domain type, the type belongs in a shared module that both depend on, never in one layer’s implementation. The reverse arrangement creates a circular dependency wearing the costume of a seam, and the circle will manifest as a build-order problem, a runtime initialization order problem, or a refactor that cascades unexpectedly. The right structure is for the shared type to live above both layers in the dependency graph, with each layer depending downward on it.

When a method does both a read and a write of state, command-query separation is being violated, and the retry semantics of any caller invoking the method become non-trivial. The corrective is to separate the command from the query and let callers compose them deliberately.

When a function takes multiple parameters of the same primitive type and the parameters are semantically distinct, newtype wrappers should be introduced. The compiler will then catch argument-order mistakes that would otherwise reach runtime.

When a struct has nullable fields whose nullability is correlated with other fields’ values, the type should be refactored into an enum with variants that capture the correlated combinations. The combinations the code does not want to handle then become combinations the type cannot represent.

When inbound data is checked for validity and passed downstream in its raw form, parsing has been mistaken for validation. The corrective is to introduce a domain type whose existence implies validity, with a fallible parser as its only constructor, and to let downstream code receive the parsed type rather than the raw input.

### 8.2 An Onboarding Exercise

For teams transitioning to a language with stronger seam enforcement, particularly Rust, an exercise that consistently produces the desired insight is to port a well-tested module from the existing codebase and observe what happens to the test suite.

The exercise proceeds as follows. Select a module from the existing codebase that has a substantial test suite and that exercises several seam-creating patterns: dependency injection, interface-based abstraction, and ideally some validation logic at its boundary. Port the module to the target language, preserving the architecture but adopting the target language’s idioms for each pattern. Then port the test suite. As each test is ported, classify it into one of three categories: tests that port directly, tests that port with modification, and tests that become impossible to write because the bug they were guarding against can no longer compile.

The third category is the payoff that the exercise is designed to surface. In a typical port from TypeScript or Python to Rust, between 15% and 40% of the test suite falls into the third category, depending on how heavily the original module relied on runtime type checking, null-checking, and string-stringly-typed identifiers. Each test in the third category is a class of bug that the type system now prevents, and the absence of those tests is recovered effort that compounds across the lifetime of the system. The team’s collective insight that “this is what the type system is doing for us” is more durable than any argument made in a document, because the team has experienced it directly.

The exercise should be timed-boxed at one week for a module of moderate size. The result is not intended to be production code; the result is intended to be experience. After the exercise, the team is in a substantively different position to evaluate the language choice and the seam discipline that accompanies it.

-----

## 9. Relationship to v1 and v2

This document occupies the methodology position in a three-document set. The v1 document (`a2a-bridge-analysis.md`) is the analysis and recommendation for a specific system. The v2 document (`a2a-bridge-ecosystem.md`) is the ecosystem taxonomy and the layered-stack framing that places the v1 recommendation in context. The v3 document is the engineering methodology that underlies both. Each document is intended to stand alone for its own audience while being citable from the others.

The substantive claims that the methodology supports in v1 and v2 are worth being explicit about. The v1 recommendation for Rust rested partly on rubric scoring and partly on the seam-enforcement argument; this document develops the seam-enforcement argument in full and makes it portable. The v2 recommendation to fork `agent-client-protocol-conductor` as the architectural spine rested partly on convenience and partly on the conductor’s proxy-chain pattern being a physical embodiment of the seam discipline; this document gives the underlying pattern catalog that makes the latter argument concrete. The v2 increment plan’s annotation of natural extraction points rested on the assumption that the bridge’s internal seams would be preserved during initial construction; this document specifies exactly what that preservation requires.

The methodology applies beyond the bridge. Charter platform engineering’s longer-lived projects — Platypus, the strangler-fig migration in Vulcan, the AI-driven test planning platform, the various polyglot service migrations — all share the property that they will outlive their original designers and will undergo changes the original designers did not anticipate. The seam discipline is the engineering posture that makes such systems survive those changes with their architectural integrity intact. Adopting the discipline as a Charter-wide platform-engineering practice has compounding returns that are visible only at the multi-year horizon.

The discipline is also one that aligns naturally with practices the team already has. Dependency injection and service abstraction, which the team uses for testability, are the foundation. The remaining patterns are extensions of the same principle applied to different scales and different concerns. The transition from where the team is to fully internalized seam discipline is not a paradigm shift; it is a continuation of the practices the team already values, made more comprehensive and, in the case of language choice, more mechanically enforced.

-----

## Appendix A — Reference Reading

The substantive sources for the patterns described in this document, listed for those who want to read the primary literature rather than this synthesis.

Michael Feathers, *Working Effectively with Legacy Code* (2004), introduces the seam concept and its application to testability of legacy systems. The book remains the canonical reference for the underlying idea and is worth reading in full.

Alistair Cockburn, “Hexagonal Architecture” (2005, originally published as “Ports and Adapters”), introduces the hexagonal architecture pattern. The article is freely available and concise.

Eric Evans, *Domain-Driven Design: Tackling Complexity in the Heart of Software* (2003), is the primary source for the anti-corruption layer pattern and for the broader vocabulary of strategic design that the seam discipline operates within.

Robert C. Martin, *Clean Architecture: A Craftsman’s Guide to Software Structure and Design* (2017), develops the clean architecture variant of hexagonal architecture. The book is opinionated; reading it alongside Cockburn’s original article provides useful balance.

Martin Fowler’s site (martinfowler.com) hosts authoritative articles on the strangler fig pattern, the tolerant reader pattern, and many others referenced in this document.

Alexis King, “Parse, Don’t Validate” (2019), is a short essay that introduces the principle and is the most-cited modern source for the pattern. Available on her blog.

Joe Duffy and others on phantom types and typestate have produced significant literature in the OCaml, F#, and Rust communities; the Rust documentation’s chapter on advanced types includes a typestate example that is a useful starting point.

Yaron Minsky’s “Effective ML” series of talks from Jane Street articulates the make-illegal-states-unrepresentable principle and is the source from which much of the modern Rust and TypeScript adoption of the pattern descends.

Michael Nygard’s article “Documenting Architecture Decisions” (2011) introduces the ADR format and is the source most teams adopting ADRs cite. The format has evolved in the years since but the essential structure remains.

Melvin Conway, “How Do Committees Invent?” (1968), is the original paper articulating what is now called Conway’s Law. It is short and historically interesting, though the modern application of the law to software architecture has developed substantially beyond Conway’s original observations.

-----

*End of document. Reads in series with `a2a-bridge-analysis.md` (v1) and `a2a-bridge-ecosystem.md` (v2).*