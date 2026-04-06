# Shared OAuth Onboarding Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract ~1,500 lines of duplicated OAuth onboarding logic from `codex.rs` and `gemini.rs` into a shared `oauth.rs` flow engine parameterized by a typed `OAuthOnboardingSpec`.

**Architecture:** A plain `OAuthOnboardingSpec` struct with static data and `fn` pointers defines provider-specific identity, env vars, and hooks (config resolution, profile building, config persistence). A shared flow engine in `oauth.rs` owns flow state, browser/CLI lifecycle, profile-store match-and-upsert, and Control UI integration. Provider modules shrink to static spec definitions and thin hook implementations.

**Tech Stack:** Rust, serde_json, tokio, oauth2, reqwest, sha2, hex, uuid

**Spec:** `docs/superpowers/specs/2026-04-05-shared-oauth-onboarding-design.md`

---

### Task 1: Create oauth.rs with types and shared helpers

**Files:**
- Create: `src/onboarding/oauth.rs`
- Modify: `src/onboarding/mod.rs`

This task creates the new module with all type definitions (OAuthOnboardingSpec, flow state, typed results) and moves the duplicated helper functions (callback_html, escape_html, format_oauth_provider_error, now_ms, validate_and_persist_config) from codex.rs/gemini.rs into the shared module.

- [ ] **Step 1: Create `src/onboarding/oauth.rs` with the spec struct and all type definitions**

```rust
//! Shared OAuth onboarding flow engine.
//!
//! Owns the browser flow, CLI fallback, Control flow status/apply, and
//! OAuth-profile persistence lifecycle. Provider-specific config resolution,
//! profile construction, and config mutation live in typed `fn` hooks on
//! `OAuthOnboardingSpec`.

use serde_json::Value;
use std::path::Path;

use crate::auth::profiles::{
    AuthProfile, AuthProfileCredentialKind, OAuthProvider, OAuthProviderConfig, OAuthTokens,
    ProfileStore, StoredOAuthProviderConfig, UserInfo,
};

// ─── Spec ───────────────────────────────────���───────────────────────────────

/// Provider-specific configuration for the shared OAuth onboarding engine.
/// One `static` instance per OAuth-capable provider.
pub(crate) struct OAuthOnboardingSpec {
    // Identity
    pub oauth_provider: OAuthProvider,
    pub display_name: &'static str,
    pub idp_display_name: &'static str,
    pub provider_label: &'static str,

    // Env var names
    pub client_id_env: &'static str,
    pub client_secret_env: &'static str,

    // Per-provider flow limits
    pub max_pending_flows: usize,
    pub flow_ttl_secs: u64,

    // Provider hooks (fn pointers)
    pub resolve_provider_config: fn(
        cfg: &Value,
        client_id_override: Option<String>,
        client_secret_override: Option<String>,
        redirect_uri: String,
        state_dir: &Path,
    ) -> Result<OAuthProviderConfig, String>,

    pub build_auth_profile: fn(
        tokens: OAuthTokens,
        provider_config: &OAuthProviderConfig,
        user_info: UserInfo,
    ) -> AuthProfile,

    pub write_provider_config: fn(cfg: &mut Value, profile_id: &str, client_id: &str),
}

// ─── Flow State ──────────────────���──────────────────────────────────────��───

#[derive(Clone)]
pub(crate) struct OAuthCompletion {
    pub client_id: String,
    pub auth_profile: AuthProfile,
}

#[derive(Clone)]
pub(crate) enum OAuthFlowState {
    Pending,
    InProgress,
    Completed(Box<OAuthCompletion>),
    Failed(String),
}

#[derive(Clone)]
pub(crate) struct PendingOAuthFlow {
    pub id: String,
    pub state: String,
    pub code_verifier: String,
    pub provider_config: OAuthProviderConfig,
    pub created_at_ms: u64,
    pub flow_state: OAuthFlowState,
    pub spec: &'static OAuthOnboardingSpec,
}

// ─── Typed Results ──────────────────────────────────────────────────────────

pub(crate) struct OAuthStartResult {
    pub flow_id: String,
    pub auth_url: String,
    pub redirect_uri: String,
}

pub(crate) enum OAuthStatusResult {
    Pending,
    InProgress,
    Completed {
        profile_name: String,
        email: Option<String>,
    },
    Failed {
        error: String,
    },
    NotFound,
}

pub(crate) struct OAuthApplyResult {
    pub profile_id: String,
    pub client_id: String,
}
```

Add the shared helper functions below the types (these are currently duplicated verbatim in both codex.rs and gemini.rs):

```rust
// ─── Shared Helpers ─────────────────────────────────────────────────────────

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn callback_html(title: &str, body: &str) -> String {
    let title = escape_html(title);
    let body = escape_html(body);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1><p>{}</p></body></html>",
        title, title, body
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub(crate) fn format_oauth_provider_error(
    error: &str,
    error_description: Option<&str>,
) -> String {
    match error_description.filter(|d| !d.trim().is_empty()) {
        Some(desc) => format!("{error}: {desc}"),
        None => error.to_string(),
    }
}

pub(crate) fn validate_and_persist_config(config: &Value) -> Result<(), String> {
    let config_path = crate::config::resolve_config_path();
    crate::config::schema::validate_and_write(config, &config_path)
        .map_err(|err| format!("config validation failed: {err}"))
}
```

- [ ] **Step 2: Register the module in `src/onboarding/mod.rs`**

Add `pub mod oauth;` to the module list.

- [ ] **Step 3: Run `cargo check`**

Run: `scripts/cargo-serial check`
Expected: Compiles with no errors (types defined but not yet used — dead_code warnings expected).

- [ ] **Step 4: Commit**

```bash
git add src/onboarding/oauth.rs src/onboarding/mod.rs
git commit -m "feat(onboarding): add oauth.rs types and shared helpers (#201)"
```

---

### Task 2: Shared flow storage with per-provider limits and expiry

**Files:**
- Modify: `src/onboarding/oauth.rs`

Add the global flow map, per-provider insert with limits, lookup by state, lookup by flow_id, and expiry cleanup.

- [ ] **Step 1: Add flow storage functions to oauth.rs**

```rust
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::sync::RwLock;

static OAUTH_FLOWS: LazyLock<RwLock<HashMap<String, PendingOAuthFlow>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Insert a new flow, enforcing per-provider pending limits.
pub(crate) fn insert_oauth_flow(flow: PendingOAuthFlow) -> Result<(), String> {
    let mut flows = OAUTH_FLOWS.blocking_write();
    let provider_label = flow.spec.provider_label;
    let max = flow.spec.max_pending_flows;

    let count = flows
        .values()
        .filter(|f| f.spec.provider_label == provider_label)
        .count();
    if count >= max {
        return Err(format!(
            "too many pending {} OAuth flows (max {max})",
            flow.spec.display_name
        ));
    }

    flows.insert(flow.id.clone(), flow);
    Ok(())
}

/// Look up a pending flow by its OAuth `state` parameter.
/// Returns None if not found or expired.
pub(crate) fn find_flow_by_state(
    spec: &'static OAuthOnboardingSpec,
    state_param: &str,
) -> Option<PendingOAuthFlow> {
    let flows = OAUTH_FLOWS.blocking_read();
    flows
        .values()
        .find(|f| {
            std::ptr::eq(f.spec, spec) && f.state == state_param
        })
        .cloned()
}

/// Get a flow by its ID. Returns None if not found.
pub(crate) fn get_flow(flow_id: &str) -> Option<PendingOAuthFlow> {
    let flows = OAUTH_FLOWS.blocking_read();
    flows.get(flow_id).cloned()
}

/// Update a flow's state.
pub(crate) fn update_flow_state(flow_id: &str, new_state: OAuthFlowState) {
    let mut flows = OAUTH_FLOWS.blocking_write();
    if let Some(flow) = flows.get_mut(flow_id) {
        flow.flow_state = new_state;
    }
}

/// Remove expired flows for all providers.
pub(crate) fn cleanup_expired_flows() {
    let mut flows = OAUTH_FLOWS.blocking_write();
    let now = now_ms();
    flows.retain(|_, flow| {
        let ttl_ms = flow.spec.flow_ttl_secs * 1000;
        now.saturating_sub(flow.created_at_ms) < ttl_ms
    });
}
```

- [ ] **Step 2: Run `cargo check`**

Run: `scripts/cargo-serial check`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/onboarding/oauth.rs
git commit -m "feat(onboarding): add shared OAuth flow storage with per-provider limits (#201)"
```

---

### Task 3: Shared match-and-upsert profile persistence

**Files:**
- Modify: `src/onboarding/oauth.rs`

Extract the profile-store match-and-upsert pattern that is currently duplicated in `upsert_openai_profile()` and `upsert_google_profile()`.

- [ ] **Step 1: Add `upsert_oauth_profile` to oauth.rs**

```rust
/// Shared match-and-upsert: loads the profile store, finds an existing profile
/// by provider/user_id/email to preserve its id and created_at_ms, then upserts.
pub(crate) fn upsert_oauth_profile(
    state_dir: &Path,
    profile: AuthProfile,
) -> Result<String, String> {
    let store =
        ProfileStore::from_env(state_dir.to_path_buf()).map_err(|err| err.to_string())?;
    store.load().map_err(|err| err.to_string())?;

    let existing = store.find_matching(
        profile.provider,
        profile.user_id.as_deref(),
        profile.email.as_deref(),
    );
    let profile = if let Some(existing) = existing {
        AuthProfile {
            id: existing.id,
            created_at_ms: existing.created_at_ms,
            ..profile
        }
    } else {
        profile
    };
    let id = profile.id.clone();
    store.upsert(profile).map_err(|err| err.to_string())?;
    Ok(id)
}
```

- [ ] **Step 2: Run `cargo check`**

Run: `scripts/cargo-serial check`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/onboarding/oauth.rs
git commit -m "feat(onboarding): add shared OAuth match-and-upsert profile persistence (#201)"
```

---

### Task 4: Shared flow engine — start, callback, status, apply

**Files:**
- Modify: `src/onboarding/oauth.rs`

Implement the four Control-facing flow engine functions. These are the core of the shared engine, extracted from the parallel implementations in codex.rs and gemini.rs.

- [ ] **Step 1: Add `require_encrypted_profile_store` helper**

```rust
/// Verify that CARAPACE_CONFIG_PASSWORD is set (required for encrypted profile storage).
pub(crate) fn require_encrypted_profile_store(
    spec: &OAuthOnboardingSpec,
) -> Result<(), String> {
    if std::env::var("CARAPACE_CONFIG_PASSWORD").is_err() {
        return Err(format!(
            "{} sign-in requires CARAPACE_CONFIG_PASSWORD to encrypt stored credentials.",
            spec.display_name
        ));
    }
    Ok(())
}
```

- [ ] **Step 2: Add `start_oauth_flow`**

```rust
use crate::auth::profiles::generate_auth_url;

pub(crate) fn start_oauth_flow(
    spec: &'static OAuthOnboardingSpec,
    cfg: &Value,
    client_id_override: Option<String>,
    client_secret_override: Option<String>,
    redirect_base_url: &str,
) -> Result<OAuthStartResult, String> {
    require_encrypted_profile_store(spec)?;

    let redirect_uri = format!(
        "{}/control/onboarding/{}/callback",
        redirect_base_url.trim_end_matches('/'),
        spec.provider_label
    );
    let provider_config = (spec.resolve_provider_config)(
        cfg,
        client_id_override,
        client_secret_override,
        redirect_uri.clone(),
        &crate::paths::resolve_state_dir(),
    )?;

    let state = format!("{}-{}", spec.provider_label, uuid::Uuid::new_v4());
    let flow_id = uuid::Uuid::new_v4().to_string();
    let (auth_url, code_verifier) =
        generate_auth_url(&provider_config, &state).map_err(|err| err.to_string())?;

    let flow = PendingOAuthFlow {
        id: flow_id.clone(),
        state,
        code_verifier,
        provider_config,
        created_at_ms: now_ms(),
        flow_state: OAuthFlowState::Pending,
        spec,
    };
    insert_oauth_flow(flow)?;

    Ok(OAuthStartResult {
        flow_id,
        auth_url,
        redirect_uri,
    })
}
```

- [ ] **Step 3: Add `complete_oauth_callback`**

This is the async function called when the OAuth provider redirects back. It looks up the flow by OAuth `state` parameter, exchanges the authorization code for tokens, fetches user info, and transitions the flow to Completed or Failed.

```rust
use crate::auth::profiles::{exchange_code, fetch_user_info};

pub(crate) async fn complete_oauth_callback(
    spec: &'static OAuthOnboardingSpec,
    state_param: &str,
    code: Option<&str>,
    error: Option<&str>,
    error_description: Option<&str>,
) -> Result<(), String> {
    cleanup_expired_flows();

    // Find the flow by state and transition to InProgress
    let flow = find_flow_by_state(spec, state_param)
        .ok_or_else(|| {
            format!(
                "No pending {} sign-in matches this callback. The flow may have expired.",
                spec.display_name
            )
        })?;
    let flow_id = flow.id.clone();

    // Check current state
    match &flow.flow_state {
        OAuthFlowState::Completed(_) => return Ok(()),
        OAuthFlowState::Failed(err) => return Err(err.clone()),
        OAuthFlowState::InProgress => {
            return Err(format!(
                "{} sign-in callback is already being processed. \
                 Return to the Control UI and refresh status.",
                spec.display_name
            ))
        }
        OAuthFlowState::Pending => {}
    }

    update_flow_state(&flow_id, OAuthFlowState::InProgress);

    // Handle OAuth error response
    if let Some(err) = error.filter(|v| !v.trim().is_empty()) {
        let msg = format_oauth_provider_error(err, error_description);
        update_flow_state(&flow_id, OAuthFlowState::Failed(msg.clone()));
        return Err(msg);
    }

    // Exchange code for tokens
    let code = match code.map(str::trim).filter(|v| !v.is_empty()) {
        Some(c) => c,
        None => {
            let msg = "Missing OAuth authorization code".to_string();
            update_flow_state(&flow_id, OAuthFlowState::Failed(msg.clone()));
            return Err(msg);
        }
    };

    let result = async {
        let tokens = exchange_code(&flow.provider_config, code, &flow.code_verifier)
            .await
            .map_err(|err| err.to_string())?;
        let userinfo = fetch_user_info(
            spec.oauth_provider,
            &flow.provider_config,
            &tokens.access_token,
        )
        .await
        .map_err(|err| err.to_string())?;
        let auth_profile =
            (spec.build_auth_profile)(tokens, &flow.provider_config, userinfo);
        Ok::<OAuthCompletion, String>(OAuthCompletion {
            client_id: flow.provider_config.client_id.clone(),
            auth_profile,
        })
    }
    .await;

    match result {
        Ok(completion) => {
            update_flow_state(
                &flow_id,
                OAuthFlowState::Completed(Box::new(completion)),
            );
            Ok(())
        }
        Err(msg) => {
            update_flow_state(&flow_id, OAuthFlowState::Failed(msg.clone()));
            Err(msg)
        }
    }
}
```

- [ ] **Step 4: Add `oauth_flow_status`**

```rust
pub(crate) fn oauth_flow_status(flow_id: &str) -> OAuthStatusResult {
    let flow = match get_flow(flow_id) {
        Some(f) => f,
        None => return OAuthStatusResult::NotFound,
    };
    match &flow.flow_state {
        OAuthFlowState::Pending => OAuthStatusResult::Pending,
        OAuthFlowState::InProgress => OAuthStatusResult::InProgress,
        OAuthFlowState::Completed(completion) => OAuthStatusResult::Completed {
            profile_name: completion.auth_profile.name.clone(),
            email: completion.auth_profile.email.clone(),
        },
        OAuthFlowState::Failed(err) => OAuthStatusResult::Failed {
            error: err.clone(),
        },
    }
}
```

- [ ] **Step 5: Add `apply_oauth_flow`**

```rust
pub(crate) fn apply_oauth_flow(
    flow_id: &str,
    state_dir: &Path,
    cfg: &mut Value,
) -> Result<OAuthApplyResult, String> {
    let flow = get_flow(flow_id)
        .ok_or_else(|| "OAuth flow not found or expired".to_string())?;

    let completion = match flow.flow_state {
        OAuthFlowState::Completed(c) => *c,
        OAuthFlowState::Failed(err) => return Err(err),
        _ => return Err("OAuth flow has not completed yet".to_string()),
    };

    let profile_id = upsert_oauth_profile(state_dir, completion.auth_profile)?;
    (flow.spec.write_provider_config)(cfg, &profile_id, &completion.client_id);
    validate_and_persist_config(cfg)?;

    // Remove the flow now that it's been applied
    {
        let mut flows = OAUTH_FLOWS.blocking_write();
        flows.remove(flow_id);
    }

    Ok(OAuthApplyResult {
        profile_id,
        client_id: completion.client_id,
    })
}
```

- [ ] **Step 6: Run `cargo check`**

Run: `scripts/cargo-serial check`
Expected: Compiles (functions defined but not yet called from outside the module).

- [ ] **Step 7: Commit**

```bash
git add src/onboarding/oauth.rs
git commit -m "feat(onboarding): add shared OAuth flow engine (start/callback/status/apply) (#201)"
```

---

### Task 5: Shared CLI OAuth flow

**Files:**
- Modify: `src/onboarding/oauth.rs`

Add `run_cli_oauth` and `persist_cli_oauth` — the CLI-side OAuth flow that spawns a localhost callback server and opens the browser.

- [ ] **Step 1: Add `persist_cli_oauth`**

```rust
pub(crate) fn persist_cli_oauth(
    spec: &'static OAuthOnboardingSpec,
    completion: OAuthCompletion,
    state_dir: &Path,
    config: &mut Value,
) -> Result<String, String> {
    let profile_id = upsert_oauth_profile(state_dir, completion.auth_profile)?;
    (spec.write_provider_config)(config, &profile_id, &completion.client_id);
    validate_and_persist_config(config)?;
    Ok(profile_id)
}
```

- [ ] **Step 2: Add `run_cli_oauth`**

This is the CLI fallback: binds a localhost listener, generates the auth URL, prints it for the user, waits for the OAuth callback, then exchanges the code and builds the completion. Extracted from the nearly-identical `run_cli_openai_oauth_with_timeout` / `run_cli_google_oauth_with_timeout`.

```rust
pub(crate) async fn run_cli_oauth(
    spec: &'static OAuthOnboardingSpec,
    cfg: &Value,
    client_id_override: Option<String>,
    client_secret_override: Option<String>,
) -> Result<OAuthCompletion, String> {
    run_cli_oauth_with_timeout(
        spec,
        cfg,
        client_id_override,
        client_secret_override,
        Duration::from_secs(300),
    )
    .await
}

async fn run_cli_oauth_with_timeout(
    spec: &'static OAuthOnboardingSpec,
    cfg: &Value,
    client_id_override: Option<String>,
    client_secret_override: Option<String>,
    timeout: Duration,
) -> Result<OAuthCompletion, String> {
    require_encrypted_profile_store(spec)?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("failed to bind local OAuth callback listener: {err}"))?;
    let bind_addr = listener
        .local_addr()
        .map_err(|err| format!("failed to determine local OAuth callback port: {err}"))?;
    let redirect_uri = format!("http://127.0.0.1:{}/auth/callback", bind_addr.port());
    let provider_config = (spec.resolve_provider_config)(
        cfg,
        client_id_override,
        client_secret_override,
        redirect_uri.clone(),
        &crate::paths::resolve_state_dir(),
    )?;

    let parsed_redirect = url::Url::parse(&provider_config.redirect_uri)
        .map_err(|err| format!("invalid {} OAuth redirect URI: {err}", spec.display_name))?;
    let host = parsed_redirect.host_str().unwrap_or_default();
    if host != "127.0.0.1" && host != "localhost" {
        return Err(format!(
            "CLI {} sign-in requires a loopback redirect URI; use Control UI sign-in.",
            spec.idp_display_name
        ));
    }

    let path = parsed_redirect.path().to_string();
    let state = format!("{}-cli-{}", spec.provider_label, uuid::Uuid::new_v4());
    let (auth_url, verifier) =
        generate_auth_url(&provider_config, &state).map_err(|err| err.to_string())?;

    println!();
    println!(
        "Open this URL to sign in with {} for {}:",
        spec.idp_display_name, spec.display_name
    );
    println!("{auth_url}");
    println!();
    println!("Waiting for OAuth callback on {}{} ...", bind_addr, path);

    // Spawn the local callback server.
    // The full callback handler logic (accept connection, parse query params,
    // exchange code, build completion) follows the same pattern as the existing
    // codex/gemini CLI flows. This is the one large block that gets extracted
    // from both run_cli_openai_oauth_with_timeout and run_cli_google_oauth_with_timeout.
    let spec_ref = spec;
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let server_task = tokio::spawn(async move {
        loop {
            let accept = tokio::select! {
                result = listener.accept() => result,
                _ = shutdown_rx.changed() => return,
            };
            let (stream, _) = match accept {
                Ok(conn) => conn,
                Err(_) => continue,
            };
            let expected_path = path.clone();
            let expected_state = state.clone();
            let verifier = verifier.clone();
            let provider_config = provider_config.clone();

            let result = handle_cli_oauth_connection(
                spec_ref,
                stream,
                &expected_path,
                &expected_state,
                &verifier,
                &provider_config,
            )
            .await;

            if let Some(completion_result) = result {
                let _ = result_tx.send(completion_result);
                return;
            }
        }
    });

    let result = match tokio::time::timeout(timeout, result_rx).await {
        Ok(Ok(Ok(completion))) => Ok(completion),
        Ok(Ok(Err(err))) => Err(err),
        Ok(Err(_)) => Err(format!(
            "{} OAuth callback channel closed unexpectedly",
            spec.display_name
        )),
        Err(_) => Err(format!(
            "Timed out waiting for {} sign-in callback",
            spec.display_name
        )),
    };

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;

    result
}
```

Note: `handle_cli_oauth_connection` is a helper that reads the HTTP request from the TCP stream, parses query parameters, validates the state, exchanges the code, and returns the completion. This is extracted from the inline logic in both codex.rs and gemini.rs. The implementation reads the raw HTTP request, extracts `code`/`state`/`error` params, calls `exchange_code` + `fetch_user_info` + `spec.build_auth_profile`, writes an HTML response, and returns the result.

- [ ] **Step 3: Add `handle_cli_oauth_connection` helper**

Extract the shared TCP-level callback handler from the inline logic in codex.rs:360-455 / gemini.rs:382-477. This function:
1. Reads raw HTTP request from the TCP stream
2. Parses query params (code, state, error, error_description)
3. Validates state matches expected
4. On error: writes error HTML response, returns Err
5. On success: exchanges code for tokens, fetches user info, builds completion
6. Writes success HTML response
7. Returns `Some(Result<OAuthCompletion, String>)` or `None` if the request wasn't for the callback path

The implementation follows the existing pattern exactly — the only parameterization is through `spec` for display names and `spec.build_auth_profile` for the profile hook.

- [ ] **Step 4: Run `cargo check`**

Run: `scripts/cargo-serial check`
Expected: Compiles.

- [ ] **Step 5: Commit**

```bash
git add src/onboarding/oauth.rs
git commit -m "feat(onboarding): add shared CLI OAuth flow (run_cli_oauth/persist_cli_oauth) (#201)"
```

---

### Task 6: Define Codex spec and hooks, migrate codex.rs

**Files:**
- Modify: `src/onboarding/codex.rs` — replace flow logic with spec + hooks, keep public API surface
- Modify: `src/server/control.rs` — migrate Codex OAuth handlers to use shared engine

This is the first provider migration. After this task, Codex onboarding uses the shared engine while Gemini still uses its own implementation.

- [ ] **Step 1: Define `CODEX_SPEC` and hook functions in codex.rs**

Keep `resolve_openai_oauth_provider_config` as the resolve hook (it has provider-specific env var names and config path logic). Extract the profile builder and config writer as standalone functions matching the `fn` pointer signatures.

The static spec:

```rust
pub(crate) static CODEX_SPEC: crate::onboarding::oauth::OAuthOnboardingSpec =
    crate::onboarding::oauth::OAuthOnboardingSpec {
        oauth_provider: OAuthProvider::OpenAI,
        display_name: "Codex",
        idp_display_name: "OpenAI",
        provider_label: "codex",
        client_id_env: "OPENAI_OAUTH_CLIENT_ID",
        client_secret_env: "OPENAI_OAUTH_CLIENT_SECRET",
        max_pending_flows: 20,
        flow_ttl_secs: 30 * 60,
        resolve_provider_config: resolve_openai_oauth_provider_config_hook,
        build_auth_profile: build_openai_auth_profile_hook,
        write_provider_config: write_openai_oauth_config,
    };
```

The three hook implementations wrap the existing logic. `resolve_openai_oauth_provider_config_hook` delegates to the existing `resolve_openai_oauth_provider_config`. `build_openai_auth_profile_hook` contains the SHA256 ID generation with "openai-" prefix, "Codex ({display})" name format, and `OAuthProvider::OpenAI`. `write_openai_oauth_config` contains the `ensure_openai_oauth_config` logic (auth.profiles.enabled, auth.profiles.providers.openai.clientId, codex.authProfile).

- [ ] **Step 2: Rewrite Codex public functions as thin delegates**

Replace `start_control_openai_oauth`, `complete_control_openai_oauth_callback`, `control_openai_oauth_status`, `apply_control_openai_oauth`, `run_cli_openai_oauth`, and `persist_cli_openai_oauth` with thin wrappers that delegate to the shared engine with `&CODEX_SPEC`.

For example:

```rust
pub fn start_control_openai_oauth(
    cfg: &Value,
    client_id_override: Option<String>,
    client_secret_override: Option<String>,
    redirect_base_url: &str,
) -> Result<CodexOAuthStart, String> {
    let result = crate::onboarding::oauth::start_oauth_flow(
        &CODEX_SPEC,
        cfg,
        client_id_override,
        client_secret_override,
        redirect_base_url,
    )?;
    Ok(CodexOAuthStart {
        flow_id: result.flow_id,
        auth_url: result.auth_url,
        redirect_uri: result.redirect_uri,
    })
}
```

Keep `CodexOAuthStart`, `CodexOAuthStatus`, `CodexOAuthCompletion` types as thin wrappers that map from the shared `OAuthStartResult`/`OAuthStatusResult`/`OAuthCompletion` types — this preserves the existing public API and avoids changing all call sites in one step.

- [ ] **Step 3: Remove dead code from codex.rs**

Delete the now-unused private functions: `PendingCodexOAuthFlow`, `CodexOAuthFlowState`, `CODEX_OAUTH_FLOWS`, `insert_openai_oauth_flow`, `begin_control_openai_oauth_completion`, `finish_control_openai_oauth_flow`, `cleanup_expired_flows`, `now_ms`, `callback_html`, `escape_html`, `format_oauth_provider_error`, `validate_and_persist_config`, `upsert_openai_profile`, `ensure_openai_oauth_config`, `build_openai_auth_profile`, `run_cli_openai_oauth_with_timeout`, the inline CLI server logic.

- [ ] **Step 4: Migrate Codex control.rs handlers**

Update `codex_oauth_start_handler`, `codex_oauth_status_handler`, `codex_oauth_apply_handler`, and `codex_oauth_callback_handler` in `src/server/control.rs` to call through the existing Codex thin wrappers (which now delegate to the shared engine). No signature changes needed — the handlers already call `crate::onboarding::codex::*`.

- [ ] **Step 5: Run tests**

Run: `scripts/cargo-serial nextest run -p carapace --filter-expr 'test(codex) | test(onboarding) | test(control)'`
Expected: All existing Codex onboarding tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/onboarding/codex.rs src/onboarding/oauth.rs src/server/control.rs
git commit -m "refactor(onboarding): migrate Codex OAuth to shared engine (#201)"
```

---

### Task 7: Define Gemini spec and hooks, migrate gemini.rs

**Files:**
- Modify: `src/onboarding/gemini.rs` — replace OAuth flow logic with spec + hooks, keep API-key path intact
- Modify: `src/server/control.rs` — migrate Gemini OAuth handlers

Same pattern as Task 6, but for Gemini. The API-key path (`validate_gemini_api_key_input`, `validate_gemini_base_url_input`, `write_gemini_api_key_config`) stays in gemini.rs unchanged.

- [ ] **Step 1: Define `GEMINI_SPEC` and hook functions in gemini.rs**

```rust
pub(crate) static GEMINI_SPEC: crate::onboarding::oauth::OAuthOnboardingSpec =
    crate::onboarding::oauth::OAuthOnboardingSpec {
        oauth_provider: OAuthProvider::Google,
        display_name: "Gemini",
        idp_display_name: "Google",
        provider_label: "gemini",
        client_id_env: "GOOGLE_OAUTH_CLIENT_ID",
        client_secret_env: "GOOGLE_OAUTH_CLIENT_SECRET",
        max_pending_flows: 20,
        flow_ttl_secs: 30 * 60,
        resolve_provider_config: resolve_google_oauth_provider_config_hook,
        build_auth_profile: build_google_auth_profile_hook,
        write_provider_config: write_google_oauth_config,
    };
```

Hooks follow the same pattern as Codex: `build_google_auth_profile_hook` uses "google-" prefix, "Gemini ({display})" name, `OAuthProvider::Google`. `write_google_oauth_config` sets auth.profiles.providers.google.clientId, google.authProfile, and removes google.apiKey (the API-key/OAuth mutual exclusion).

- [ ] **Step 2: Rewrite Gemini OAuth public functions as thin delegates**

Same pattern as Task 6 Step 2. Keep `GeminiOAuthStart`, `GeminiOAuthStatus`, `GeminiOAuthCompletion` as thin mapping types.

- [ ] **Step 3: Remove dead OAuth code from gemini.rs, keep API-key path**

Delete the OAuth-specific private functions (same list as Task 6 Step 3 but for Gemini variants). Keep `validate_gemini_api_key_input`, `validate_gemini_base_url_input`, `write_gemini_api_key_config`, and `apply_control_gemini_api_key` untouched.

- [ ] **Step 4: Run tests**

Run: `scripts/cargo-serial nextest run -p carapace --filter-expr 'test(gemini) | test(onboarding) | test(control)'`
Expected: All existing Gemini onboarding tests pass (both OAuth and API-key paths).

- [ ] **Step 5: Commit**

```bash
git add src/onboarding/gemini.rs src/onboarding/oauth.rs src/server/control.rs
git commit -m "refactor(onboarding): migrate Gemini OAuth to shared engine (#201)"
```

---

### Task 8: Migrate CLI integration and update test helpers

**Files:**
- Modify: `src/cli/mod.rs` — update `configure_codex_provider_interactive` and `configure_gemini_provider_interactive`
- Modify: `src/onboarding/oauth.rs` — add test helper for inserting completed flows

- [ ] **Step 1: Verify CLI integration compiles and works**

The CLI functions `configure_codex_provider_interactive` and `configure_gemini_provider_interactive` in `src/cli/mod.rs` already call through the public Codex/Gemini wrappers (`run_cli_openai_oauth`, `persist_cli_openai_oauth`, etc.) which now delegate to the shared engine. Verify no changes are needed — the public API surface was preserved in Tasks 6-7.

Run: `scripts/cargo-serial check`
Expected: Compiles with no errors.

- [ ] **Step 2: Add shared test helper for inserting completed flows**

Both codex.rs and gemini.rs had `insert_completed_control_*_oauth_flow_for_test()` helpers used by control.rs tests. Add a shared version in oauth.rs:

```rust
#[cfg(test)]
pub(crate) fn insert_completed_flow_for_test(
    spec: &'static OAuthOnboardingSpec,
) -> String {
    let flow_id = uuid::Uuid::new_v4().to_string();
    let completion = OAuthCompletion {
        client_id: "test-client-id".to_string(),
        auth_profile: AuthProfile {
            id: format!("{}-test", spec.provider_label),
            name: format!("{} (Test)", spec.display_name),
            provider: spec.oauth_provider,
            user_id: Some("test-user".to_string()),
            email: Some("test@example.com".to_string()),
            display_name: Some("Test User".to_string()),
            avatar_url: None,
            created_at_ms: now_ms(),
            last_used_ms: None,
            credential_kind: AuthProfileCredentialKind::OAuth,
            tokens: None,
            token: None,
            oauth_provider_config: None,
        },
    };
    let flow = PendingOAuthFlow {
        id: flow_id.clone(),
        state: format!("{}-test-state", spec.provider_label),
        code_verifier: "test-verifier".to_string(),
        provider_config: OAuthProviderConfig::default_for_test(),
        created_at_ms: now_ms(),
        flow_state: OAuthFlowState::Completed(Box::new(completion)),
        spec,
    };
    insert_oauth_flow(flow).expect("test flow insert");
    flow_id
}
```

Update `insert_completed_control_openai_oauth_flow_for_test` in codex.rs and `insert_completed_control_google_oauth_flow_for_test` in gemini.rs to delegate to this shared helper.

- [ ] **Step 3: Run full test suite for affected modules**

Run: `scripts/cargo-serial nextest run -p carapace --filter-expr 'test(codex) | test(gemini) | test(onboarding) | test(control) | test(cli)'`
Expected: All tests pass.

- [ ] **Step 4: Run clippy**

Run: `scripts/cargo-serial clippy --all-targets`
Expected: No warnings in modified files.

- [ ] **Step 5: Commit**

```bash
git add src/onboarding/ src/cli/mod.rs src/server/control.rs
git commit -m "refactor(onboarding): migrate CLI integration and test helpers to shared OAuth engine (#201)"
```

---

### Task 9: Final cleanup and verification

**Files:**
- All modified files from Tasks 1-8

- [ ] **Step 1: Verify no dead code remains**

Run: `scripts/cargo-serial check 2>&1 | grep 'warning.*dead_code\|warning.*unused'`
Expected: No dead_code warnings in onboarding/, control.rs, or cli/mod.rs.

- [ ] **Step 2: Verify line count reduction**

Run: `wc -l src/onboarding/codex.rs src/onboarding/gemini.rs src/onboarding/oauth.rs`
Expected: codex.rs ~150 lines, gemini.rs ~400 lines, oauth.rs ~600 lines. Total should be significantly less than the original ~2,700 lines.

- [ ] **Step 3: Run full project test suite**

Run: `scripts/cargo-serial nextest run -p carapace`
Expected: All tests pass, no regressions.

- [ ] **Step 4: Run cargo fmt**

Run: `scripts/cargo-serial fmt --all -- --check`
Expected: No formatting issues.

- [ ] **Step 5: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "chore(onboarding): final cleanup for shared OAuth extraction (#201)"
```
