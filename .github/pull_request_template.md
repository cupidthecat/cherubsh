## What changed

Describe the focused behavior or maintenance change and link the issue it closes.

## Public seam

State how a user, script, C client, package consumer, or terminal session observes this change.

## Test evidence

List the focused red and green tests, then the broader commands you ran.

- [ ] Focused test added or updated
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] Relevant workspace or parity gate
- [ ] Documentation updated when public behavior changed

## Notes

Call out remaining limits, intentional skips, compatibility decisions, or follow-up work.
