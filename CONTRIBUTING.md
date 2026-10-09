# Contributing to Open Download Manager

Thanks for helping. Bug reports, fixes and features are all welcome.

## Before you start

- For anything larger than a small fix, open an issue first so we can agree on the approach.
- Engine changes need tests: `cargo test -p odm-core` must pass.
- UI changes: `npm --prefix ui run check` must report 0 errors.
- Follow the design spec in `download-manager-ui/HANDOFF.md`: tokens only, no new colors or fonts.

## Contributor terms

By submitting a contribution (a pull request, patch or other code) you agree that:

1. You wrote it, or otherwise have the right to submit it under these terms.
2. Your contribution is licensed to the public under the project license, AGPL-3.0-only.
3. You also grant the project maintainer (@zbreilly) a perpetual, worldwide, non-exclusive, royalty-free, irrevocable license to use, modify, sublicense and relicense your contribution, including under other open-source licenses. This lets the project change or add licenses in the future without tracking down every contributor.
4. You keep the copyright to your contribution.

Add a `Signed-off-by: Your Name <email>` line to your commits (`git commit -s`) to confirm these terms.
