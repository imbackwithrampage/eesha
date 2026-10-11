// __NAME__ — a shared service (docs/ARCHITECTURE.md §8). Not a feature: the
// features that need it list this crate in their own Cargo.toml. Refcounted:
// nothing here runs without a live subscriber, and the handle `subscribe`
// returns unsubscribes on drop.
