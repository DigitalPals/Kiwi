# Contributing to Kiwi

Thanks for helping. Kiwi is a small project, so a short conversation first saves work:

- **Bugs:** open an issue with what you did, what you expected and what happened. Include
  your macOS and iOS versions, how Tailscale is installed (Homebrew or the Tailscale app) and
  the relevant lines of `~/Library/Logs/Kiwi/kiwi.log`. Remove device ids, tailnet names and
  anything else personal first.
- **Features:** open an issue describing the problem before sending a large change.
- **Security problems:** don't open an issue; see [SECURITY.md](SECURITY.md).

## Pull requests

1. Set up a checkout as described in [docs/development.md](docs/development.md).
2. Keep each pull request to one change, with tests for new behaviour.
3. Make sure everything passes before you push:

   ```sh
   cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
   (cd web && npm run typecheck && npm test)
   python3 -m pytest automation/test_runner.py
   shellcheck install.sh uninstall.sh deploy/deploy.sh
   ```

4. When you change the HTTP API, update [docs/api.md](docs/api.md), `web/src/api/types.ts`
   and the mock API in `web/mock/` together.
5. Update the docs a change affects (the guides in `docs/` and the README).

Use example data in tests and fixtures only (`example.com` logins, `*.example.ts.net`
names, team `A1B2C3D4E5`, made-up UDIDs). Never commit real device ids, Apple team or
certificate details, tailnet names, keys or logs.

Kiwi Builds recipes (`automation/recipes.json`) pin every source to a full commit hash; a
recipe change should say what changed upstream and how you tested the build on a Mac.

By contributing you agree that your contributions are licensed under the project's
[MIT license](LICENSE).
