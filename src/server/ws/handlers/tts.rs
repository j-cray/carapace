//! Text-to-speech handlers.
//!
//! Manages TTS settings including provider selection, voice configuration,
//! and text-to-speech conversion via the OpenAI TTS API.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

use super::super::*;

/// Available TTS providers
pub const TTS_PROVIDERS: [&str; 4] = ["system", "elevenlabs", "openai", "google"];

/// Audio formats supported by the OpenAI TTS API.
pub const OPENAI_AUDIO_FORMATS: [&str; 4] = ["mp3", "opus", "aac", "flac"];

/// Available system voices (platform-dependent)
pub const SYSTEM_VOICES: [&str; 6] = ["samantha", "alex", "victoria", "karen", "daniel", "moira"];

/// Global TTS state
static TTS_STATE: LazyLock<RwLock<TtsState>> = LazyLock::new(|| RwLock::new(TtsState::default()));

/// Shared HTTP client for TTS requests
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// TTS configuration state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsState {
    /// Whether TTS is enabled
    pub enabled: bool,
    /// Active TTS provider
    pub provider: Option<String>,
    /// Selected voice for the current provider
    pub voice: Option<String>,
    /// Speaking rate (0.5 to 2.0, 1.0 is normal)
    pub rate: f64,
    /// Pitch adjustment (-1.0 to 1.0, 0.0 is normal)
    pub pitch: f64,
    /// Volume (0.0 to 1.0)
    pub volume: f64,
    /// Provider-specific API keys (stored securely in production)
    pub provider_config: ProviderConfig,
    /// Current speech ID (if a speak call is active)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_speech_id: Option<String>,
}

impl Default for TtsState {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            voice: None,
            rate: 1.0,
            pitch: 0.0,
            volume: 1.0,
            provider_config: ProviderConfig::default(),
            current_speech_id: None,
        }
    }
}

/// Provider-specific configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// ElevenLabs configuration
    pub elevenlabs: Option<ElevenLabsConfig>,
    /// OpenAI configuration
    pub openai: Option<OpenAiTtsConfig>,
}

/// ElevenLabs TTS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElevenLabsConfig {
    pub voice_id: Option<String>,
    pub model_id: Option<String>,
    pub stability: f64,
    pub similarity_boost: f64,
}

/// OpenAI TTS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiTtsConfig {
    pub model: String,
    pub voice: String,
}

/// Get TTS status
pub(super) fn handle_tts_status() -> Result<Value, ErrorShape> {
    let state = TTS_STATE.read();
    Ok(json!({
        "enabled": state.enabled,
        "provider": state.provider,
        "voice": state.voice,
        "rate": state.rate,
        "pitch": state.pitch,
        "volume": state.volume
    }))
}

/// List available TTS providers
pub(super) fn handle_tts_providers() -> Result<Value, ErrorShape> {
    let state = TTS_STATE.read();
    let providers: Vec<Value> = TTS_PROVIDERS
        .iter()
        .map(|&p| {
            json!({
                "id": p,
                "name": match p {
                    "system" => "System Voice",
                    "elevenlabs" => "ElevenLabs",
                    "openai" => "OpenAI TTS",
                    "google" => "Google Cloud TTS",
                    _ => p
                },
                "available": p == "system" || p == state.provider.as_deref().unwrap_or("")
            })
        })
        .collect();

    Ok(json!({
        "providers": providers,
        "current": state.provider
    }))
}

/// Enable TTS
pub(super) fn handle_tts_enable() -> Result<Value, ErrorShape> {
    let mut state = TTS_STATE.write();
    state.enabled = true;

    // Default to configured provider or system if none set
    if state.provider.is_none() {
        let mut default_provider = "system".to_string();
        if let Ok(cfg) = config::load_config() {
            if let Some(dp) = cfg
                .get("talk")
                .and_then(|v| v.get("defaultProvider"))
                .and_then(|v| v.as_str())
            {
                if TTS_PROVIDERS.contains(&dp) {
                    default_provider = dp.to_string();
                }
            }
        }
        state.provider = Some(default_provider);
    }

    Ok(json!({
        "ok": true,
        "enabled": true,
        "provider": state.provider
    }))
}

/// Disable TTS
pub(super) fn handle_tts_disable() -> Result<Value, ErrorShape> {
    let mut state = TTS_STATE.write();
    state.enabled = false;

    Ok(json!({
        "ok": true,
        "enabled": false
    }))
}

/// Resolve the OpenAI API key from config or environment.
fn resolve_openai_api_key() -> Option<String> {
    // Try config first: models.providers.openai.apiKey
    if let Ok(cfg) = config::load_config() {
        if let Some(key) = cfg
            .get("models")
            .and_then(|v| v.get("providers"))
            .and_then(|v| v.get("openai"))
            .and_then(|v| v.get("apiKey"))
            .and_then(|v| v.as_str())
        {
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }

    // Fall back to environment variable
    env::var("OPENAI_API_KEY").ok().filter(|k| !k.is_empty())
}

/// Validate and normalise the requested audio format.
///
/// Returns the format string to use with the OpenAI API. If `raw` is `None`
/// the default format `"mp3"` is returned.
fn validate_audio_format(
    raw: Option<&str>,
    channel: Option<&str>,
) -> Result<&'static str, ErrorShape> {
    match raw {
        None => {
            if channel.unwrap_or("").eq_ignore_ascii_case("signal") {
                Ok("opus")
            } else {
                Ok("mp3")
            }
        }
        Some(f) => {
            let lower = f.trim();
            OPENAI_AUDIO_FORMATS
                .iter()
                .find(|&&fmt| fmt.eq_ignore_ascii_case(lower))
                .copied()
                .ok_or_else(|| {
                    error_shape(
                        ERROR_INVALID_REQUEST,
                        &format!(
                            "unsupported audio format '{}'; supported: mp3, opus, aac, flac",
                            f
                        ),
                        Some(json!({ "supportedFormats": OPENAI_AUDIO_FORMATS })),
                    )
                })
        }
    }
}

/// Call the OpenAI TTS API and return raw audio bytes.
async fn openai_tts_request(
    api_key: &str,
    text: &str,
    voice: &str,
    format: &str,
    speed: f64,
) -> Result<bytes::Bytes, ErrorShape> {
    let body = json!({
        "model": "tts-1",
        "input": text,
        "voice": voice,
        "response_format": format,
        "speed": speed.clamp(0.25, 4.0)
    });

    let response = HTTP_CLIENT
        .post("https://api.openai.com/v1/audio/speech")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            error_shape(
                ERROR_UNAVAILABLE,
                &format!("OpenAI TTS request failed: {}", e),
                None,
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let err_body = response.text().await.unwrap_or_default();
        return Err(error_shape(
            ERROR_UNAVAILABLE,
            &format!("OpenAI TTS API error ({}): {}", status, err_body),
            None,
        ));
    }

    response.bytes().await.map_err(|e| {
        error_shape(
            ERROR_UNAVAILABLE,
            &format!("failed to read OpenAI TTS response: {}", e),
            None,
        )
    })
}

/// Call the Google Cloud TTS API and return raw audio bytes
async fn google_tts_request(
    token: &str,
    text: &str,
    voice: &str,
    format: &str,
    speed: f64,
) -> Result<bytes::Bytes, ErrorShape> {
    // Map requested format to Google Cloud TTS AudioEncoding.
    // Avoid magic strings by defining constants for supported formats.
    const ENCODING_MP3: &str = "MP3";
    const ENCODING_OGG_OPUS: &str = "OGG_OPUS";
    const ENCODING_FLAC: &str = "FLAC";
    const ENCODING_LINEAR16: &str = "LINEAR16";
    const ENCODING_MULAW: &str = "MULAW";
    const ENCODING_ALAW: &str = "ALAW";

    let audio_encoding = match format {
        "opus" | "ogg_opus" => ENCODING_OGG_OPUS,
        "flac" => ENCODING_FLAC,
        "wav" | "linear16" | "pcm" => ENCODING_LINEAR16,
        "mulaw" => ENCODING_MULAW,
        "alaw" => ENCODING_ALAW,
        "mp3" | "aac" => ENCODING_MP3,
        _ => ENCODING_MP3,
    };

    // Voice name logic. e.g. en-US-Journey-O determines language code en-US
    let mut voice_parts = voice.split('-');
    let language_code = match (voice_parts.next(), voice_parts.next()) {
        (Some(language), Some(region)) if language.len() == 2 && !region.is_empty() => {
            format!("{}-{}", language, region)
        }
        _ => "en-US".to_string(),
    };

    let body = json!({
        "input": {
            "text": text
        },
        "voice": {
            "languageCode": language_code,
            "name": voice
        },
        "audioConfig": {
            "audioEncoding": audio_encoding,
            "speakingRate": speed.clamp(0.25, 4.0)
        }
    });

    let response = HTTP_CLIENT
        .post("https://texttospeech.googleapis.com/v1/text:synthesize")
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            error_shape(
                ERROR_UNAVAILABLE,
                &format!("Google TTS request failed: {}", e),
                None,
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let err_body = response.text().await.unwrap_or_default();
        return Err(error_shape(
            ERROR_UNAVAILABLE,
            &format!("Google TTS API error ({}): {}", status, err_body),
            None,
        ));
    }

    let json: Value = response.json().await.map_err(|e| {
        error_shape(
            ERROR_UNAVAILABLE,
            &format!("failed to parse Google TTS response: {}", e),
            None,
        )
    })?;

    let base64_audio = json
        .get("audioContent")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            error_shape(
                ERROR_UNAVAILABLE,
                "Google TTS response missing audioContent",
                None,
            )
        })?;

    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(base64_audio)
        .map(bytes::Bytes::from)
        .map_err(|e| {
            error_shape(
                ERROR_UNAVAILABLE,
                &format!("failed to decode Google TTS audioContent: {}", e),
                None,
            )
        })
}

/// Convert text to speech.
///
/// Params:
///   - `text` (required): the text to convert.
///   - `format` (optional): audio format — `mp3` (default), `opus`, `aac`, or `flac`.
///
/// When the provider is `openai` and an API key is available the handler
/// calls the OpenAI TTS API and returns base64-encoded audio.  If no key is
/// configured for the `openai` provider the handler returns a clear error.
/// For other providers (e.g. `system`) the handler returns `audio: null`
/// since those providers do not have a server-side synthesis path.
pub(super) async fn handle_tts_convert(params: Option<&Value>) -> Result<Value, ErrorShape> {
    let text = params
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| error_shape(ERROR_INVALID_REQUEST, "text is required", None))?;

    if text.trim().is_empty() {
        return Err(error_shape(
            ERROR_INVALID_REQUEST,
            "text cannot be empty",
            None,
        ));
    }

    let requested_format = params
        .and_then(|v| v.get("format"))
        .and_then(|v| v.as_str());
    let channel = params
        .and_then(|v| v.get("channel"))
        .and_then(|v| v.as_str());
    let audio_format = validate_audio_format(requested_format, channel)?;

    // Read state in a block so the parking_lot guard (which is !Send) is
    // dropped before any await point.
    let (provider, voice, rate, pitch) = {
        let state = TTS_STATE.read();

        if !state.enabled {
            return Err(error_shape(ERROR_UNAVAILABLE, "TTS is not enabled", None));
        }

        let provider = state.provider.as_deref().unwrap_or("system").to_string();
        let voice = state.voice.clone().unwrap_or_else(|| "alloy".to_string());
        let rate = state.rate;
        let pitch = state.pitch;
        (provider, voice, rate, pitch)
    };

    if provider == "openai" {
        let api_key = resolve_openai_api_key().ok_or_else(|| {
            error_shape(
                ERROR_UNAVAILABLE,
                "OpenAI API key not configured; set models.providers.openai.apiKey in config or OPENAI_API_KEY env var",
                None,
            )
        })?;

        let audio_bytes = openai_tts_request(&api_key, text, &voice, audio_format, rate).await?;

        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_bytes);

        return Ok(json!({
            "ok": true,
            "text": text,
            "provider": provider,
            "voice": voice,
            "rate": rate,
            "pitch": pitch,
            "audio": audio_b64,
            "audioFormat": audio_format,
            "audioSize": audio_bytes.len(),
            "duration": null
        }));
    } else if provider == "google" {
        let token = crate::gcp::resolve_gcp_adc_token(&HTTP_CLIENT)
            .await
            .map_err(|e| error_shape(ERROR_UNAVAILABLE, &e, None))?;

        // Use a default voice for Google if none was set or if defaulting occurred to alloy
        let gcp_voice = if voice == "alloy" {
            let mut default_voice = "en-US-Journey-O".to_string();
            if let Ok(cfg) = config::load_config() {
                if let Some(v) = cfg
                    .get("google")
                    .and_then(|val| val.get("tts"))
                    .and_then(|val| val.get("voice"))
                    .and_then(|val| val.as_str())
                {
                    if !v.trim().is_empty() {
                        default_voice = v.trim().to_string();
                    }
                }
            }
            default_voice
        } else {
            voice.clone()
        };

        // Ensure format mapping is clear; if AAC or something is requested, google_tts_request defaults it to MP3.
        let actual_format = if audio_format == "aac" {
            "mp3"
        } else {
            audio_format
        };
        let audio_bytes = google_tts_request(&token, text, &gcp_voice, actual_format, rate).await?;

        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_bytes);

        return Ok(json!({
            "ok": true,
            "text": text,
            "provider": provider,
            "voice": gcp_voice,
            "rate": rate,
            "pitch": pitch,
            "audio": audio_b64,
            "audioFormat": actual_format,
            "audioSize": audio_bytes.len(),
            "duration": null
        }));
    }

    // Non-OpenAI and Non-Google provider: no server-side synthesis available.
    Ok(json!({
        "ok": true,
        "text": text,
        "provider": provider,
        "voice": voice,
        "rate": rate,
        "pitch": pitch,
        "audio": null,
        "audioFormat": audio_format,
        "duration": null
    }))
}

/// Set TTS provider
pub(super) fn handle_tts_set_provider(params: Option<&Value>) -> Result<Value, ErrorShape> {
    let provider = params
        .and_then(|v| v.get("provider"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| error_shape(ERROR_INVALID_REQUEST, "provider is required", None))?;

    if !TTS_PROVIDERS.contains(&provider) {
        return Err(error_shape(
            ERROR_INVALID_REQUEST,
            &format!("unknown provider: {}", provider),
            Some(json!({ "validProviders": TTS_PROVIDERS })),
        ));
    }

    let mut state = TTS_STATE.write();
    state.provider = Some(provider.to_string());

    // Reset voice when changing provider
    state.voice = None;

    Ok(json!({
        "ok": true,
        "provider": provider
    }))
}

/// Set TTS voice
pub(super) fn handle_tts_set_voice(params: Option<&Value>) -> Result<Value, ErrorShape> {
    let voice = params
        .and_then(|v| v.get("voice"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| error_shape(ERROR_INVALID_REQUEST, "voice is required", None))?;

    let mut state = TTS_STATE.write();
    state.voice = Some(voice.to_string());

    Ok(json!({
        "ok": true,
        "voice": voice,
        "provider": state.provider
    }))
}

/// List available voices for the current provider
pub(super) fn handle_tts_voices() -> Result<Value, ErrorShape> {
    let state = TTS_STATE.read();
    let provider = state.provider.as_deref().unwrap_or("system");

    let voices: Vec<Value> = match provider {
        "system" => SYSTEM_VOICES
            .iter()
            .map(|&v| {
                json!({
                    "id": v,
                    "name": v.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() + &v[1..],
                    "gender": match v {
                        "samantha" | "victoria" | "karen" | "moira" => "female",
                        _ => "male"
                    }
                })
            })
            .collect(),
        "openai" => vec!["alloy", "echo", "fable", "onyx", "nova", "shimmer"]
            .into_iter()
            .map(|v| {
                json!({
                    "id": v,
                    "name": v.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default() + &v[1..]
                })
            })
            .collect(),
        "google" => vec![
            "en-US-Journey-D", "en-US-Journey-F", "en-US-Journey-O",
            "en-US-Neural2-A", "en-US-Neural2-C", "en-US-Neural2-D",
            "en-US-Neural2-E", "en-US-Neural2-F", "en-US-Neural2-G"
        ]
            .into_iter()
            .map(|v| {
                json!({
                    "id": v,
                    "name": v
                })
            })
            .collect(),
        _ => vec![],
    };

    Ok(json!({
        "provider": provider,
        "voices": voices,
        "current": state.voice
    }))
}

/// Configure TTS settings (rate, pitch, volume)
pub(super) fn handle_tts_configure(params: Option<&Value>) -> Result<Value, ErrorShape> {
    let mut state = TTS_STATE.write();

    if let Some(rate) = params.and_then(|v| v.get("rate")).and_then(|v| v.as_f64()) {
        state.rate = rate.clamp(0.5, 2.0);
    }

    if let Some(pitch) = params.and_then(|v| v.get("pitch")).and_then(|v| v.as_f64()) {
        state.pitch = pitch.clamp(-1.0, 1.0);
    }

    if let Some(volume) = params
        .and_then(|v| v.get("volume"))
        .and_then(|v| v.as_f64())
    {
        state.volume = volume.clamp(0.0, 1.0);
    }

    Ok(json!({
        "ok": true,
        "rate": state.rate,
        "pitch": state.pitch,
        "volume": state.volume
    }))
}

/// Stop any ongoing TTS playback
pub(super) fn handle_tts_stop() -> Result<Value, ErrorShape> {
    let mut state = TTS_STATE.write();
    let stopped_id = state.current_speech_id.take();
    Ok(json!({
        "ok": true,
        "stopped": stopped_id.is_some(),
        "speechId": stopped_id
    }))
}

/// Speak text immediately (shorthand for convert + play).
///
/// Delegates to the conversion pipeline and wraps the result with a unique
/// speech ID so callers can track playback.
pub(super) async fn handle_tts_speak(params: Option<&Value>) -> Result<Value, ErrorShape> {
    let text = params
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| error_shape(ERROR_INVALID_REQUEST, "text is required", None))?;

    if text.trim().is_empty() {
        return Err(error_shape(
            ERROR_INVALID_REQUEST,
            "text cannot be empty",
            None,
        ));
    }

    // Generate a unique ID for this speech request
    let speech_id = uuid::Uuid::new_v4().to_string();
    {
        let mut state = TTS_STATE.write();
        state.current_speech_id = Some(speech_id.clone());
    }

    // Delegate to the convert pipeline for the actual audio synthesis.
    let mut converted = handle_tts_convert(params).await?;

    // Attach the speech ID and mark as playing.
    if let Some(obj) = converted.as_object_mut() {
        obj.insert("speechId".to_string(), json!(speech_id));
        obj.insert("status".to_string(), json!("playing"));
    }

    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Mutex to serialize tests that modify global state
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset_state() {
        // Ensure tests don't read the user's actual config to keep test results deterministic.
        std::env::set_var("CARAPACE_CONFIG_PATH", "nonexistent_config_for_test.json5");
        let mut state = TTS_STATE.write();
        *state = TtsState::default();
    }

    #[test]
    fn test_tts_status_default() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        let result = handle_tts_status().unwrap();
        assert_eq!(result["enabled"], false);
        assert!(result["provider"].is_null());
        assert_eq!(result["rate"], 1.0);
    }

    #[test]
    fn test_tts_enable_disable() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let result = handle_tts_enable().unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["enabled"], true);
        assert_eq!(result["provider"], "system");

        let result = handle_tts_disable().unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["enabled"], false);
    }

    #[test]
    fn test_tts_set_provider() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({ "provider": "openai" });
        let result = handle_tts_set_provider(Some(&params)).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["provider"], "openai");
    }

    #[test]
    fn test_tts_set_invalid_provider() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({ "provider": "invalid" });
        let result = handle_tts_set_provider(Some(&params));
        assert!(result.is_err());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_requires_enabled() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({ "text": "Hello world" });
        let result = handle_tts_convert(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_UNAVAILABLE);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_when_enabled() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({ "text": "Hello world" });
        let result = handle_tts_convert(Some(&params)).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["text"], "Hello world");
        assert_eq!(result["provider"], "system");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_openai_no_api_key_returns_error() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        // Enable TTS and set provider to openai
        handle_tts_enable().unwrap();
        let params = json!({ "provider": "openai" });
        handle_tts_set_provider(Some(&params)).unwrap();

        // Ensure no OPENAI_API_KEY is set for this test
        env::remove_var("OPENAI_API_KEY");

        let params = json!({ "text": "Hello from OpenAI" });
        let result = handle_tts_convert(Some(&params)).await;
        assert!(
            result.is_err(),
            "should error when no API key is configured"
        );
        let err = result.unwrap_err();
        assert_eq!(err.code, ERROR_UNAVAILABLE);
        assert!(
            err.message.contains("API key"),
            "error message should mention API key: {}",
            err.message
        );
    }

    #[test]
    fn test_tts_providers() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        let result = handle_tts_providers().unwrap();
        let providers = result["providers"].as_array().unwrap();
        assert!(providers.len() >= 3);
    }

    #[test]
    fn test_tts_configure() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({
            "rate": 1.5,
            "pitch": 0.2,
            "volume": 0.8
        });
        let result = handle_tts_configure(Some(&params)).unwrap();
        assert_eq!(result["rate"], 1.5);
        assert_eq!(result["pitch"], 0.2);
        assert_eq!(result["volume"], 0.8);
    }

    #[test]
    fn test_tts_configure_clamped() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({
            "rate": 10.0, // Should clamp to 2.0
            "volume": -1.0 // Should clamp to 0.0
        });
        let result = handle_tts_configure(Some(&params)).unwrap();
        assert_eq!(result["rate"], 2.0);
        assert_eq!(result["volume"], 0.0);
    }

    #[test]
    fn test_tts_voices() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        let result = handle_tts_voices().unwrap();
        assert_eq!(result["provider"], "system");
        let voices = result["voices"].as_array().unwrap();
        assert!(!voices.is_empty());
    }

    // -----------------------------------------------------------------------
    // Audio format validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_audio_format_default() {
        let result = validate_audio_format(None, None).unwrap();
        assert_eq!(result, "mp3");
    }

    #[test]
    fn test_validate_audio_format_channel_signal_defaults_to_opus() {
        let result = validate_audio_format(None, Some("signal")).unwrap();
        assert_eq!(result, "opus");
        let result = validate_audio_format(None, Some("SIGNAL")).unwrap();
        assert_eq!(result, "opus");
        let result = validate_audio_format(None, Some("discord")).unwrap();
        assert_eq!(result, "mp3");
    }

    #[test]
    fn test_validate_audio_format_all_supported() {
        for fmt in &OPENAI_AUDIO_FORMATS {
            let result = validate_audio_format(Some(fmt), None).unwrap();
            assert_eq!(result, *fmt);
        }
    }

    #[test]
    fn test_validate_audio_format_case_insensitive() {
        assert_eq!(validate_audio_format(Some("MP3"), None).unwrap(), "mp3");
        assert_eq!(validate_audio_format(Some("Opus"), None).unwrap(), "opus");
        assert_eq!(validate_audio_format(Some("AAC"), None).unwrap(), "aac");
        assert_eq!(validate_audio_format(Some("FLAC"), None).unwrap(), "flac");
    }

    #[test]
    fn test_validate_audio_format_invalid() {
        let result = validate_audio_format(Some("wav"), None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ERROR_INVALID_REQUEST);
        assert!(err.message.contains("wav"));
    }

    // -----------------------------------------------------------------------
    // Convert with format parameter tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_invalid_format() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({ "text": "Hello", "format": "wav" });
        let result = handle_tts_convert(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_INVALID_REQUEST);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_system_returns_null_audio() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({ "text": "Hello", "format": "opus" });
        let result = handle_tts_convert(Some(&params)).await.unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["provider"], "system");
        assert!(result["audio"].is_null());
        assert_eq!(result["audioFormat"], "opus");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_empty_text() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({ "text": "   " });
        let result = handle_tts_convert(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_INVALID_REQUEST);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_convert_missing_text() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({});
        let result = handle_tts_convert(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_INVALID_REQUEST);
    }

    // -----------------------------------------------------------------------
    // Speak handler tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_speak_requires_enabled() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({ "text": "Hello" });
        let result = handle_tts_speak(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_UNAVAILABLE);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_speak_returns_speech_id() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({ "text": "Hello" });
        let result = handle_tts_speak(Some(&params)).await.unwrap();
        assert_eq!(result["ok"], true);
        assert!(result["speechId"].is_string(), "should have speechId");
        assert_eq!(result["status"], "playing");
        assert_eq!(result["text"], "Hello");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_speak_empty_text() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();

        let params = json!({ "text": "" });
        let result = handle_tts_speak(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_INVALID_REQUEST);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn test_tts_speak_openai_no_key_returns_error() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        handle_tts_enable().unwrap();
        let p = json!({ "provider": "openai" });
        handle_tts_set_provider(Some(&p)).unwrap();
        env::remove_var("OPENAI_API_KEY");

        let params = json!({ "text": "Test speech" });
        let result = handle_tts_speak(Some(&params)).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ERROR_UNAVAILABLE);
    }

    // -----------------------------------------------------------------------
    // Set voice tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tts_set_voice() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({ "voice": "echo" });
        let result = handle_tts_set_voice(Some(&params)).unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["voice"], "echo");
    }

    #[test]
    fn test_tts_set_voice_empty_rejected() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();

        let params = json!({ "voice": "  " });
        let result = handle_tts_set_voice(Some(&params));
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // OpenAI voices listing
    // -----------------------------------------------------------------------

    #[test]
    fn test_tts_voices_openai() {
        let _lock = TEST_LOCK.lock().unwrap();
        reset_state();
        let p = json!({ "provider": "openai" });
        handle_tts_set_provider(Some(&p)).unwrap();

        let result = handle_tts_voices().unwrap();
        assert_eq!(result["provider"], "openai");
        let voices = result["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 6);
        let ids: Vec<&str> = voices.iter().map(|v| v["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"alloy"));
        assert!(ids.contains(&"shimmer"));
    }
}
