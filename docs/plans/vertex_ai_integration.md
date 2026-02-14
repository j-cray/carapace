# Plan: Add Google Vertex AI Backend to Carapace

## Overview

This plan outlines the steps to integrate Google's Vertex AI as a **standalone backend provider** for Carapace. This integration will allow users to utilize models like `gemini-2.5-flash` via their Google Cloud Platform (GCP) projects, adhering to Carapace's security and distribution guidelines.

## Goals

- **Independence**: Create a distinct `VertexProvider` separate from the existing `GeminiProvider` (AI Studio).
- **Security**: Implement robust, **auto-refreshing authentication** (GCP ADC/Metadata) instead of short-lived static tokens.
- **Distribution**: Maintain a lightweight dependency footprint (avoiding heavy GCP SDKs).
- **Functionality**: Support streaming content generation from Vertex AI models.
- **Testing**: Verification using `gemini-2.5-flash`.

## Architecture & Design

### 1. Configuration

We will extend the `Simple Agent Config` and `Global Config` to support Vertex AI specific parameters.

- **New Config Fields**:
  - `vertex_project_id`: The GCP Project ID.
  - `vertex_location`: The GCP Region (e.g., `us-central1`).
- **Environment Variables**:
  - `VERTEX_PROJECT_ID`
  - `VERTEX_LOCATION` (Default: `us-central1`)

### 2. Provider Implementation (`src/agent/vertex.rs`)

We will create a new file `src/agent/vertex.rs` implementing `LlmProvider`.

- **Struct Definition**:

  ```rust
  struct CachedToken {
      token: String,
      expires_at: std::time::Instant,
  }

  pub struct VertexProvider {
      client: reqwest::Client,
      project_id: String,
      location: String,
      token_manager: Arc<dyn TokenProvider + Send + Sync>,
      // Cache with interior mutability for thread-safe updates
      token_cache: Arc<tokio::sync::RwLock<Option<CachedToken>>>,
      base_url: String,
  }
  ```

- **Vertex AI Base URL**:
    `https://{location}-aiplatform.googleapis.com/v1beta1/projects/{project_id}/locations/{location}/publishers/google/models/{model}:streamGenerateContent`

- **Authentication Strategy (Robust, Async, Cached)**:

  We will implement an `async trait TokenProvider` with two lightweight implementations. Crucially, the provider will **cache** tokens to avoid latency on every request.

  1. **`GCloudCliProvider`**: Wraps the `gcloud auth print-access-token` command.
      - **Mechanism**: Spawns `tokio::process::Command` to avoid blocking the runtime.
      - **Use Case**: Local development / Desktop users.

  2. **`MetadataProvider`**: Queries the GCP Metadata Server (`http://metadata.google.internal/...`).
      - **Mechanism**: Async HTTP request via `reqwest`.
      - **Use Case**: Production / Cloud deployments.

  The provider will attempt to use `GCloudCliProvider` first. If `gcloud` is not found (`NotFound`), it falls back to `MetadataProvider`. If `gcloud` exists but fails (`ExecutionError`), it errors out immediately to aid debugging.

- **Code Reuse**:

  - While the provider struct is separate, the JSON payload for Gemini models on Vertex AI is largely compatible with AI Studio. We may duplicates some logic from `gemini.rs` (e.g., `build_body`, `parse_sse`) or extract shared logic into `gemini_common.rs` if the overlap is significant. **Decision**: Copy-paste-modify for now to ensure total independence as requested, unless it becomes unwieldy.

### 3. Dependency Management

- **No New Heavy Dependencies**: We will NOT add `google-cloud-aiplatform` crate.
- **HTTP Client**: Continue using `reqwest`.
- **Process Execution**: Use `tokio::process::Command` (for non-blocking I/O).

## Implementation Steps

### Step 1: Implement `src/agent/vertex.rs` with Async Auth & Caching

- Define `async trait TokenProvider`.
- Implement `GCloudCliProvider`:
  - Run `tokio::process::Command::new("gcloud")`.
  - Capture stdout, trim whitespace.
  - Return `Result<String, TokenError>`.
  - Distinguish `std::io::ErrorKind::NotFound` (fallback) from other errors (fatal).
- Implement `MetadataProvider`:
  - `reqwest::get` to metadata server.
  - **MUST** include header `Metadata-Flavor: Google`.
- Implement `VertexProvider`:
  - `token_cache: Arc<tokio::sync::RwLock<Option<CachedToken>>>`.
  - `get_token(&self)` method:
    - Read lock: check if token exists AND `expiry > now + buffer`. Return if valid.
    - Write lock: double-check (to handle race), then call `token_provider.fetch_token()`.
    - Update cache with new token + calculated expiry (default 1 hour if unknown).
  - `complete`: Calls `self.get_token().await` -> Injects `Authorization: Bearer <token>`.

### Step 2: Register Provider in `src/agent/mod.rs` & `provider.rs`

- Add `pub mod vertex;` to `src/agent/mod.rs`.
- Update `MultiProvider` in `src/agent/provider.rs`:
  - Add `vertex: Option<Arc<dyn LlmProvider>>` field.
  - Add `with_vertex` builder method.
  - Update `has_any_provider`.
  - Update `Debug` impl.

### Step 3: Implement Routing Logic

Update `MultiProvider::select_provider`:

- **Routing Rule**: If model starts with `vertex/` or `vertex:`, route to `VertexProvider`.
- **Prefix Stripping**: Strip `vertex/` prefix before sending the model name to the API.
  - Example: User requests `vertex/gemini-2.5-flash` -> API receives `gemini-2.5-flash`.

### Step 4: Update Configuration

- Update `src/config/schema.rs` (or equivalent) to load `VERTEX_PROJECT_ID` and `VERTEX_LOCATION`.
- Update `src/main.rs` (or factory) to initialize `VertexProvider` using the detected auth strategy.

## Security Review & Mitigations

A comprehensive security review has been conducted on this plan:

- **Token Redaction**:
  - **Risk**: Accidentally logging access tokens via `Debug` traits or `tracing`.
  - **Mitigation**: `VertexProvider` must implement `Debug` manually or use `#[debug(skip)]` on the `token_manager` field. All logging of headers must redact the `Authorization` value.

- **SSRF Prevention (Metadata Server)**:
  - **Risk**: An attacker forcing connections to the Metadata Server.
  - **Mitigation**: The `MetadataProvider` MUST enforce the `Metadata-Flavor: Google` header on all requests. This is a robust defense-in-depth measure required by GCP.

- **Command Injection**:
  - **Risk**: Using user input in shell commands.
  - **Mitigation**: The `GCloudCliProvider` must use `tokio::process::Command` with **separate arguments** (e.g., `.arg("auth").arg("print-access-token")`) rather than passing a full string to a shell. This prevents shell injection vulnerabilities.

- **Dependency Supply Chain**:
  - **Risk**: Bloated dependencies increasing attack surface.
  - **Mitigation**: Strict adherence to "No New Heavy Dependencies" policy. We only use `reqwest` and `tokio`.

## Verification Plan

### Automated Tests

- **Unit**: Test `vertex.rs` for URL construction and payload generation.
- **Integration**: Run `cargo nextest run -E 'test(test_vertex_integration)'`.

### Manual Verification

1. **Environment**:
    - Ensure `gcloud` is installed and authenticated (`gcloud auth login`).
    - set `VERTEX_PROJECT_ID` and `VERTEX_LOCATION`.

2. **Run**:

    ```bash
    cargo run --bin cara -- chat --model vertex/gemini-2.5-flash "Hello from Vertex AI with robust auth"
    ```

3. **Expected Output**: The agent responds using the Vertex AI backend, successfully acquiring a token via `gcloud` or metadata server.

## Checklists

- [ ] Implement `TokenProvider` strategies (GCloud, Metadata).
- [ ] Create `src/agent/vertex.rs` using dynamic auth.
- [ ] Integrate into `MultiProvider` and `AgentConfig`.
- [ ] Implement `vertex/` prefix routing.
- [ ] Verify with `vertex/gemini-2.5-flash`.
