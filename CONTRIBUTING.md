# Contributing

## Development Setup

1. Install Rust stable (`rustup default stable`).
2. Clone the repository.
3. Run quality checks:

```bash
cargo test --all-features
cargo bench --no-run
cargo bench -- --test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
cargo deny check
cargo audit
```

## Pull Request Checklist

- [ ] Tests added or updated for behavioral changes
- [ ] Public docs updated for API changes
- [ ] No new clippy warnings
- [ ] Does this change the public API?
- [ ] Benchmarks updated for performance-sensitive changes
