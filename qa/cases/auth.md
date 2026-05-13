# Auth & Identity Cases

verification surface of users/accounts/sessions/api_tokens + loopback bootstrap + CLI/bridge bearer auth.

## Browser

### AUTH-001 First-Load Session Bootstrap

- Suite: smoke
- Script: [`playwright/AUTH-001.spec.ts`](./playwright/AUTH-001.spec.ts)

### AUTH-002 Settings → Devices Mint + List + Rotate

- Suite: smoke
- Script: [`playwright/AUTH-002.spec.ts`](./playwright/AUTH-002.spec.ts)

## Backend

Run these test files for full PR coverage:

- [`tests/auth_qa_tests.rs`](../../tests/auth_qa_tests.rs) — setup→serve continuity, token revoke, cookie lifecycle, recovery, system-info actor, legacy config, raw-token-not-stored, boot-without-account
- [`tests/local_session_tests.rs`](../../tests/local_session_tests.rs) — loopback gate, Origin defense, disabled account, no-account 409
- [`tests/bridge_ws_tests.rs`](../../tests/bridge_ws_tests.rs) — bridge WS upgrade auth, machine_id binding, cross-bridge tampering, passthrough→enforce toggle
- [`tests/devices_tests.rs`](../../tests/devices_tests.rs) — `/api/devices/*` end-to-end (mint→410→rotate, list, kick, forget, dev-login route gating)
- [`tests/dev_auth_tests.rs`](../../tests/dev_auth_tests.rs) — dev-auth env parsing, refuse-to-start, allowlist enforcement
- [`src/server/bridge_auth.rs`](../../src/server/bridge_auth.rs) (unit) — `check()` branch matrix incl. `UserBridgeAllowed`
- [`src/store/auth/bridge_machines.rs`](../../src/store/auth/bridge_machines.rs) (unit) — bridge_machines state machine: insert / supersede / reconnect / reject / forget
- [`src/cli/bridge.rs`](../../src/cli/bridge.rs) (unit) — credentials parse, URL derivation, machine_id sanitization
- [`src/cli/credentials.rs`](../../src/cli/credentials.rs) (unit) — atomic 0600 write, bridge-credentials parity
