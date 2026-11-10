# Contributing to Kiwi

Thanks for helping. Kiwi is a small project, so a short conversation first saves work:

- **Bugs:** open an issue with what you did, what you expected and what happened. Include
  your macOS (or Linux distribution, or Docker) and iOS versions, how Kiwi and Tailscale are
  installed and the relevant lines of Kiwi's log (`~/Library/Logs/Kiwi/kiwi.log` on a Mac,
  `journalctl -u kiwi` on Linux). Remove device ids, tailnet names and anything else
  personal first.
- **Features:** open an issue describing the problem before sending a large change.
- **Security problems:** don't open an issue; see [SECURITY.md](SECURITY.md).

## Pull requests

1. Set up a checkout as described in [docs/development.md](docs/development.md).
2. Keep each pull request to one change, with tests for new behaviour.
3. Make sure everything passes before you push. These are the checks CI runs
   (`.github/workflows/ci.yml`), apart from the Docker image, the macOS job and the
   Android app:

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   cargo audit                      # cargo install cargo-audit; exceptions: .cargo/audit.toml
   (cd web && npm ci && npm run typecheck && npm test && npm run build)
   python3 -m pytest automation/test_runner.py automation/test_android.py
   shellcheck install.sh uninstall.sh deploy/deploy.sh android/scripts/build-in-docker.sh
   (cd android && ./gradlew test assembleRelease)   # when you change the Android app
   ```

   Rust's version is pinned in `rust-toolchain.toml`; update it together with the
   toolchain in `ci.yml` and the Dockerfile's `rust` image.

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
