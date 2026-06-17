# Security Remediation Plan (2026-06-17)

Context + verified findings + GitNexus impact + repair path for a security review of the MCSS backend/frontend. Execute later via subagent-driven development; verify each fix with build + tests.

---

## 1. Context

A security review surfaced findings across the file API, auth, and info-exposure surfaces. Each was **verified against the actual code** before planning (receiving-code-review discipline). Severities and verification status below. Backend = Rust/axum/sqlx; frontend = React/Vite. Auth currently uses HttpOnly cookies + CSRF double-submit (with Bearer kept for backward compat).

### Findings (verified)

| ID | Sev | Finding | Verified? | Evidence |
|----|-----|---------|-----------|----------|
| **H1** | HIGH | **File IDOR**: `get_file`, `delete_file`, `download_url`, `message_files`, `batch(delete)` take `Extension(_user)` (unused) and fetch by id with NO owner/team/conversation check. `list/search/stats` DO restrict non-admin via `uploaded_by = user.id`. Any authed user can read/sign-download/delete any file by id. | ✅ Confirmed | files/handlers.rs get_file/delete_file/download_url/message_files/batch; list uses `scope_user = (!is_admin).then(user.id)` → `WHERE (… uploaded_by = $2)` |
| **H2** | HIGH | **LINE media proxy public + unsigned**: `/api/files/line-proxy/{lineMessageId}` is on public routes, validates only that the id is numeric, then reads `line/media/{id}` and returns with public cache. Other public file routes carry an HMAC signature; this one does not. | ✅ Confirmed | files/mod.rs (public group) + handlers.rs line_proxy (numeric check only) |
| **M1** | MED | **Direct upload bypasses declared validation**: presigned creation validates contentType/size, but public `direct_upload` only verifies the signature then writes bytes; `confirm_upload` only `warn`s on size mismatch and still marks `completed`; checksum unused. | ✅ Confirmed | handlers.rs direct_upload, confirm_upload |
| **M2** | MED | **Login JSON still returns access/refresh tokens** though cookies are the transport and the frontend ignores them. | ✅ Confirmed | auth/handlers.rs login; Login.tsx | **DECISION NEEDED — see §4** |
| **M3** | MED | **Public health/config over-discloses**: public health exposes DB status + version; `/api/data-optimization/health` returns full optimization config. | ✅ Confirmed | system/handlers.rs health; system/admin.rs (data-opt health) |
| **L1** | LOW | CSRF middleware skips when no `mcss_csrf` cookie (by design, for Bearer/API clients). Add Origin/Referer check, or only skip when a Bearer header is present. | ✅ (known design) | middleware/csrf.rs |
| **L2** | LOW | CSP only `default-src 'self'`; add `base-uri`, `object-src`, `frame-ancestors`. | ✅ | middleware/security_headers |
| **U1** | UX | Sessions/Reminders load failure renders as "no data" instead of an error state. | ✅ | Sessions.tsx, Reminders.tsx |
| **U2** | UX | Session close/reopen is one-click — no confirm, no row-level busy, no double-click guard. | ✅ | Sessions.tsx |
| **U3** | UX | `mustChangePassword` only shows error text; no redirect to a change-password step. | ✅ | Login.tsx |
| **U4** | UX | Hardcoded Chinese toasts/labels coexist with `t()` i18n. | ✅ (broad) | many pages |

---

## 2. GitNexus impact (blast radius)

Ran `impact(direction: upstream)` on the fix targets:

- `delete_file` (files/handlers.rs) → **impactedCount 0** (route-handler leaf; only the router references it, not tracked as a CALL edge).
- `find` (files/store.rs) → **impactedCount 0** (qualified `store::find(...)` callers not resolved by the Rust call-graph; treated as leaf).

**Structural blast radius: LOW** — the targets are route handlers / store leaves with no tracked dependents, so changing their bodies cannot break upstream callers.

**Behavioral impact (reasoned, since the risk here is behavioral not structural):** the H1 fix adds `is_admin || uploaded_by == user.id`. The frontend reaches single-file endpoints (`fileDownloadUrl`, file panel) using fileIds obtained from the **already-scoped** `list`/`conversation_files` (which restrict non-admin to `uploaded_by`). So the fix matches existing visibility — **no frontend regression**. (Note: the existing model already only shows agents files they uploaded; a broader team/conversation-based access model is a separate future enhancement, NOT part of this security fix.)

---

## 3. Repair path (ordered)

### Phase 1 — HIGH (fix first)

**H1 — File IDOR.** In `files/handlers.rs`, add an access helper and apply it to every single-resource handler:
```rust
fn user_can_access_file(user: &AuthUser, row: &FileRow) -> bool {
    user.is_admin() || row.uploaded_by.as_deref() == Some(user.id.as_str())
}
```
- `get_file`, `download_url`, `delete_file`: after `store::find`, `if !user_can_access_file(&user, &row) { return Err(AppError::NotFound("File not found".into())) }` (404, not 403, to avoid id enumeration). Change `Extension(_user)` → `Extension(user)`.
- `batch(delete)`: for each fileId, look up the row and skip/deny ones the user can't access (admin deletes any; non-admin only own). Don't delete files the user doesn't own.
- `message_files`: add `AND (uploaded_by = $user OR <is_admin>)` to the query for non-admin (mirror `list`'s `scope_user` pattern).
- Verify: `cargo build`; `cargo test --test files` (extend/confirm a test that a non-owner non-admin gets 404 on get/download-url/delete).

**H2 — LINE media proxy.** Require authentication for `line-proxy`. Move `/api/files/line-proxy/{lineMessageId}` from the public group to the authed group (it gains `require_auth`, which reads the `mcss_access` cookie — same-origin `<img src>` sends the cookie automatically, so authenticated agent UIs still render LINE media). If a truly public URL is ever needed, switch to the HMAC-signed pattern used by the other public file routes instead. Verify: `cargo build` + `cargo test --test files` (proxy now 401 without auth).

### Phase 2 — MEDIUM

**M1 — Upload validation.** In `direct_upload`, after writing, run the same `validate_part`-style content-type/size checks the presigned/multipart path uses; reject (and clean up) on violation. In `confirm_upload`, treat a size mismatch as an ERROR (reject / mark failed), not a warning; use the checksum if present. Verify: `cargo test --test files`.

**M3 — Reduce public info.** Public `health` should return a minimal liveness payload (status only) for unauthenticated callers; keep the detailed DB/version/component info behind auth (or only in non-production). Gate `/api/data-optimization/health` detail behind admin. Verify: `cargo build` + a test asserting the public health body no longer carries DB/version.

### Phase 3 — LOW / hardening

- **L1** CSRF: add an Origin/Referer allowlist check for cookie-authenticated mutations as defense-in-depth (still skip for Bearer-only requests). 
- **L2** CSP: extend `Content-Security-Policy` with `base-uri 'self'; object-src 'none'; frame-ancestors 'none'`.

### Phase 4 — UX

- **U1** Sessions/Reminders: add an explicit `error` state; show "載入失敗，請重試" (vs empty) when the load fails.
- **U2** Session close/reopen: add a ConfirmDialog + per-row busy/disabled while the request is in flight (reuse the existing ConfirmDialog).
- **U3** Login `mustChangePassword`: route to a change-password step (or surface a clear CTA) instead of only an error string.
- **U4** i18n: (long-term) migrate hardcoded toasts/labels to `t()`. Track separately; not blocking.

---

## 4. Decisions needed

- **M2 (login JSON tokens).** Recommendation: **keep returning them** — they are the deliberate Bearer backward-compat surface (auth integration tests + any API client use the JSON tokens; the browser ignores them and relies on cookies). Fully removing them requires migrating the test suite + any Bearer client to cookie-only. Only remove if we commit to dropping Bearer support. **Awaiting user decision.**

---

## 5. Verification strategy

Per fix: backend `cargo build` + the relevant `cargo test --test {files|auth}`; frontend `tsc --noEmit` + `vitest` + `vite build`; for H1/H2 also a runtime curl check (non-owner → 404; line-proxy unauthenticated → 401). Each phase is its own commit. GitNexus impact re-checked (`detect_changes`) before committing.
