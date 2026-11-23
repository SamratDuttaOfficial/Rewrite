# Contributing to Rewrite

Thanks for helping. Bug reports, small fixes and community builds are all welcome.

## Reporting a bug

Open an issue with the **Bug report** form. Say which version and installer you used (for example
`Rewrite-2.0.3-Windows-Setup.exe`), your system, and the steps that lead to the problem. A screenshot helps.

## Building from source

See [Run from source](README.md#run-from-source) in the README. Before sending a change, run:

```
cargo test
cargo xtask selftest
```

## Pull requests

- Keep each pull request to one change, and describe what it fixes or adds.
- Match the style of the code around it.
- For a bigger feature, open an issue first so we can agree on the approach.

## Community builds

If you built an installer for a system that has no official one yet, send it in with the **Community build** form.

## Licence

Rewrite is free to use under the MIT licence with the Commons Clause (see [LICENSE](LICENSE)). By sending a
contribution you agree that it is released under the same terms.
