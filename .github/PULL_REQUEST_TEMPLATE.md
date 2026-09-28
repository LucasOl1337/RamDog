## What this changes

<!-- The problem it solves and what the user sees now. Link the issue if there is one. -->

## How it was tested

- [ ] `cargo fmt -- --check`
- [ ] `cargo test --locked -- --test-threads=1`
- [ ] `python3 tests/test_installer.py` (if `install.sh` or `linux/ramdog-launch` changed)
- [ ] Ran the app and checked the affected view (say on which OS/desktop)

## Checklist

- [ ] New UI strings exist in Portuguese and English (`locale.text`)
- [ ] Missing readings show `—`, not an invented value
- [ ] Destructive actions respect locks and protected processes
