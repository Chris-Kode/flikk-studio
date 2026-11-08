# CODE-DESIGN.md

How code in this repository is shaped. Read it before designing, writing,
refactoring or reviewing code. [AGENTS.md](AGENTS.md) holds the hard rules;
[DESIGN.md](DESIGN.md) covers the interface.

Write code at two levels:

- **Structure:** how the system is divided into modules (functions, types,
  crates) and what their interfaces look like. Each module should have a
  **simple interface** and a **powerful implementation**. The interface is the
  cost callers pay; the functionality is the benefit. Aim for benefit far
  greater than cost.
- **Details:** how the code inside each module reads: clear names, readable
  functions, no duplication, consistent formatting.

**Tie-breaking rule:** at module boundaries, depth wins. Inside the
implementation, readability wins.

---

## Part 1: Module structure (depth)

### When designing or writing new code

1. **Start from the caller.** Before writing the implementation, write the
   call site you wish existed. Keep it to as few methods and parameters as
   possible.
2. **Make the common case trivial.** Choose sensible defaults so most callers
   pass nothing extra. Optional knobs are fine; required knobs are a cost.
3. **Pull complexity downward.** If something is tricky (retries, caching,
   parsing, encoding, error recovery), handle it inside the module rather than
   pushing it to callers via flags or config.
4. **Design somewhat general-purpose.** Shape the interface around the concept
   (e.g. `config.get(key)`), not around one caller's exact sequence of steps.
5. **Hide information.** Callers should not need to know file formats, storage
   details, call ordering, or internal data structures.
6. **Define errors out of existence where reasonable.** Prefer semantics that
   make errors impossible (e.g. deleting a missing key is a no-op) over errors
   every caller must handle.

### Structural red flags

- **Shallow module:** interface is nearly as complex as the implementation.
- **Pass-through method:** a method that just forwards arguments to another
  with the same signature.
- **Classitis:** many tiny types that each do almost nothing and must be wired
  together by callers.
- **Information leakage:** the same design decision (format, protocol,
  ordering) known by several modules.
- **Temporal decomposition:** modules split by the order things happen (read,
  then parse, then validate) instead of by knowledge they hide.
- **Required call ordering:** callers must call `init()` / `open()` /
  `setup()` in the right sequence.
- **Configuration explosion:** many parameters exposed instead of choosing
  good defaults.
- **Repeated caller boilerplate:** several call sites performing the same
  multi-step dance.

---

## Part 2: Code inside modules (readability)

1. **Intention-revealing names.** Names say what something is or does, not
   how. Avoid abbreviations, noise words (`data`, `info`, `manager`) and
   misleading names. Use one word per concept consistently across the
   codebase.
2. **Readable functions.** A function should read top to bottom at a single
   level of abstraction. Keep parameter lists short; prefer 0-2 arguments, and
   group related arguments into a struct.
3. **No duplication.** Extract repeated logic into one place.
4. **No flag arguments.** A boolean parameter that switches behavior usually
   means two functions are hiding in one.
5. **Avoid side-effect surprises.** A function named `get_x` should not also
   modify state.
6. **Command-query separation.** A function either does something or answers
   something, not both.
7. **Small, focused types internally.** Private helpers and internal types can
   be small; they are not part of the public interface.
8. **Consistent formatting.** Related code stays close together; follow the
   project's existing style (`cargo fmt`).
9. **Clear error handling.** Fail with meaningful messages; don't return magic
   values like `-1` to signal errors. Use `Result` and `Option`.

---

## Part 3: Balancing rules

- **Function size:** Split a function only when each piece is independently
  understandable and has a meaningful name. Never split just to hit a line
  count. A longer function that reads clearly top to bottom is fine.
- **Comments:** Always document public interfaces: what it promises,
  parameters, units, edge cases, errors. Keep implementation comments rare;
  use them only to explain *why*, never *what*.
- **Number of types:** One responsibility per module, where a responsibility
  means *a piece of knowledge hidden*, not a single step.
- **Abstraction layers:** Every layer must hide something or transform the
  abstraction. Remove layers that only forward calls.

---

## How to refactor

- Merge shallow pieces that are always used together into one module with a
  single entry point.
- Move repeated caller boilerplate into the module.
- Replace required parameters with defaults; keep advanced options optional.
- Remove pass-through layers or give them real responsibility.
- Inside modules, rename unclear identifiers, remove duplication, and split
  flag-argument functions.
- Preserve behavior, update call sites, and note any public API changes.

Do not over-merge: if combining modules would mix unrelated responsibilities
or make the implementation hard to understand, keep them separate. Depth is
about hiding complexity, not about making modules large for their own sake.

---

## State and async work

### Make the states explicit

- Model a lifecycle as an `enum` whose variants carry their data
  (`Idle`, `Loading { read }`, `Loaded { .. }`, `Failed { at, message }`), not
  as `Option`, `bool` and counter fields that can contradict each other.
- `None` never means two things: waiting, in flight, done and failed are
  separate states.
- A cancelled job is not a failed job: don't record it as a failure or show
  it as failed.

### Async results must still be wanted

- Anything that crosses an `await`, a thread or a channel carries what it was
  for: a request counter, turn id, project id or bus version. On arrival,
  drop it if that no longer matches. Bump the counter on every new request,
  set or removal.
- Check the project after every await. A result for a project that is no
  longer open is kept for that project or dropped, never applied to the
  current one.
- A background job that proposes whole values records `ToolCtx::version()`
  when it takes its snapshot and proposes through `propose_job` /
  `execute_since`, so edits made since conflict instead of being overwritten.

### Every in-progress state has a guaranteed exit

- Workers and jobs use `catch_unwind` per item, plus a drop guard that resets
  "running" flags and wakes waiters.
- Child processes are wrapped in a guard that kills and waits on drop. A
  reader error kills the process; the rest of its output is not drained.
- A failure is retried after a back-off (about 30 s), never cached for the
  session.
- Long work takes a cancel (`&AtomicBool`, `JobProgress::cancelled()`,
  `GenerationTask`) and checks it in its loops, its waits and its retry
  sleeps.
- When media leaves the project or the project switches, its background work
  is cancelled.

### UI interactions end in one place

- An edit commits to the target it began on (see `Target` in
  `ui/inspector.rs`): GPUI delivers blur after render, when the selection may
  already have changed.
- Every drag has a single end path. A move with the button up counts as a
  release. Snap lines and hover state are cleared, and the drag is cancelled
  if its target disappears or playback starts.
- A live preview (`show_live`, `caption_live`) is cleared on every drag end,
  whether or not it commits, on a target change and on a project switch.
- UI state is keyed by ids, not list indexes (messages, words, chips, rows).
- Per-project state is reset in one function that every switch path calls
  (`switch_to`, `reset_chat_view`).

### One rule, one place

- Shared rules live in one function and are called, never re-derived: "is
  this track heard" (`Timeline::audible`), "does this clip fit this track"
  (`edit::fits_track`), "is the project dirty" (the undo position).
- Project changes go through the `CommandBus` with the right `Author`. User
  edits are `Author::User`. Agent results the user already approved use
  `execute_shared_as` with the agent as author.

### Files

- Write through a temp file and a rename. Claim a free name atomically
  (`create_new`, a hard link or a placeholder), never with `exists()`
  followed by a write.
- Never overwrite a file this operation didn't create. Remove temp and part
  files on error, and sweep the ones a crash leaves.
- Don't swallow errors with `.ok()?` on export or save paths: a render fails
  with a message rather than silently leaving something out.

---

## Before you finish

For every state you added or changed:

1. Can two of its fields contradict each other? Make it an enum.
2. What happens if the result arrives late, twice, or for another project?
3. What happens on panic, cancel and failure, and when the user switches
   project or chat?
4. Can a path leave a flag, preview, file or process behind?
5. Is this rule already written somewhere else?

For every module and function you touched:

- **Depth:** "To use this, how much must I know, compared to how much it does
  for me?" If about equal, it's shallow.
- **Readability:** "Could a new teammate read this function once and explain
  what it does?" If not, rename, restructure, or add a *why* comment.

---

## Review output format

When asked to review code against this file, respond with:

1. **Depth verdict** for each key module: Deep / Acceptable / Shallow, with one
   line of reasoning (interface cost vs. functionality provided).
2. **Structural red flags**, each with the file/function and why it matters.
3. **State and async issues** from the section above.
4. **Readability issues** (naming, duplication, flag arguments, side effects),
   grouped and prioritized.
5. **Suggested interface**: the simpler call site you recommend, before and
   after.
6. **Refactor steps**, ordered and minimal.

Put the highest-impact issues first. Structural and state problems outrank
naming and formatting.

---

## Examples

### 1. Shallow vs. deep module

❌ Callers must orchestrate reading, parsing, caching and defaults.

```rust
// Every caller repeats this:
let port = match cache.get("port") {
    Some(port) => port,
    None => {
        let port = if config_file::exists("app.json") {
            let raw = config_file::read_raw("app.json")?;
            config_file::parse_json(&raw)?["server"]["port"]
                .as_u64()
                .unwrap_or(8080)
        } else {
            8080
        };
        cache.put("port", port);
        port
    }
};
```

✅ One entry point hides files, parsing, caching, nesting and defaults.

```rust
impl Config {
    /// Returns the value at a dotted key (e.g. "server.port"), or `fallback`
    /// if absent. Loads the file lazily and caches it.
    pub fn get<T: DeserializeOwned>(&self, key: &str, fallback: T) -> T { /* ... */ }
}

let port = Config::default().get("server.port", 8080);
```

### 2. Pass-through method

❌ The layer adds interface but no functionality.

```rust
impl UserService {
    pub fn find_by_id(&self, id: UserId) -> Option<User> {
        self.repo.find_by_id(id)
    }
}
```

✅ Either call the repository directly, or give the layer a real job.

```rust
impl UserService {
    /// Returns the user, or `NotFound` if they don't exist or are deactivated.
    pub fn require_active(&self, id: UserId) -> Result<User, NotFound> {
        self.repo
            .find_by_id(id)
            .filter(|user| user.deactivated_at.is_none())
            .ok_or(NotFound(id))
    }
}
```

### 3. Required call ordering

❌ Callers must remember to connect first.

```rust
let mut client = ApiClient::new(url);
client.connect()?;
client.authenticate(&token)?;
let orders = client.fetch("/orders")?;
```

✅ The module handles setup lazily.

```rust
let client = ApiClient::new(url, token);
let orders = client.get("/orders")?; // connects and authenticates on first use
```

### 4. Configuration explosion vs. sensible defaults

❌ Every caller must decide on retry details.

```rust
http_get(url, 3, 200, 2.0, 5000, true, false);
```

✅ Defaults cover the common case; options are named and optional.

```rust
http_get(url, Request::default());
http_get(url, Request { timeout: Duration::from_secs(10), ..Default::default() });
```

### 5. Defining errors out of existence

❌ Every caller must guard against a harmless case.

```rust
if session.contains("draft") {
    session.remove("draft")?; // errors if the key is missing
}
```

✅ Removing a missing key is simply a no-op.

```rust
session.remove("draft"); // safe whether or not "draft" exists
```

### 6. Flag arguments

❌ A boolean hides two different behaviors.

```rust
render_report(&report, true);
```

✅ Two intention-revealing functions.

```rust
render_report_as_pdf(&report);
render_report_as_html(&report);
```

### 7. Names

❌

```rust
let d = get_data(u);
fn proc(lst: &[Value]) { /* ... */ }
struct UserManager { /* ... */ }
```

✅

```rust
let unpaid_invoices = find_unpaid_invoices(&customer);
fn send_overdue_reminders(invoices: &[Invoice]) { /* ... */ }
struct UserDirectory { /* ... */ }
```

### 8. Command-query separation and side effects

❌ A "getter" that also changes state.

```rust
fn get_next_ticket(queue: &mut TicketQueue) -> Option<Ticket> {
    queue.items.pop_front() // surprise mutation
}
```

✅ The name says it changes state, or the query and command are separate.

```rust
fn take_next_ticket(&mut self) -> Option<Ticket> { /* removes and returns */ }

// or
fn peek_next_ticket(&self) -> Option<&Ticket> { /* read-only */ }
fn remove_next_ticket(&mut self) { /* mutation only */ }
```

### 9. Function splitting

❌ Fragments that can't be understood on their own.

```rust
fn checkout(cart: &mut Cart) {
    step1(cart);
    step2(cart);
    step3(cart);
}
```

✅ Split only into pieces with meaningful names, or keep a clear longer
function.

```rust
fn checkout(cart: &Cart) -> Result<Order, CheckoutError> {
    let priced = apply_discounts(cart);
    let total = total_with_tax(&priced);
    place_order(priced, total)
}
```

### 10. Comments

❌ Comments repeat the code; the interface is undocumented.

```rust
// increment i by 1
i += 1;

pub fn convert(amount: i64, rate: f64) -> i64 { /* ... */ }
```

✅ The interface comment states the contract; the inline comment explains
*why*.

```rust
/// Converts an amount in cents to the target currency, in cents.
/// Rounds half-even to avoid systematic bias in batch conversions.
/// `rate` is units of target currency per unit of source; must be > 0.
pub fn convert_cents(amount_cents: i64, rate: f64) -> i64 {
    // Bankers' rounding: finance reconciles batches against this behavior.
    round_half_even(amount_cents as f64 * rate)
}
```

### 11. Error handling

❌ Magic values callers can forget to check.

```rust
fn find_index_of_user(id: UserId) -> i32 {
    users.iter().position(|u| u.id == id).map_or(-1, |i| i as i32)
}
```

✅ The type makes absence explicit, or the error is meaningful.

```rust
fn find_user(id: UserId) -> Option<&User> { /* ... */ }

fn require_user(id: UserId) -> Result<&User, NotFound> {
    find_user(id).ok_or(NotFound(id))
}
```

### 12. Explicit states

❌ Fields that can contradict each other: loading and failed at once, or
`None` meaning both "not started" and "done with nothing".

```rust
struct Waveform {
    loading: bool,
    peaks: Option<Vec<f32>>,
    error: Option<String>,
}
```

✅ One enum, each variant carrying its own data.

```rust
enum Waveform {
    Idle,
    Loading { request: u64 },
    Loaded { peaks: Vec<f32> },
    Failed { at: Instant, message: String },
}
```
