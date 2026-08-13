//! ONNX Runtime provider for CPU/NPU inference.
//!
//! Uses the `ort` crate (v2) for ONNX Runtime bindings.
//! Feature-gated behind the `onnx` feature flag.
//!
//! On Snapdragon X Elite (Windows ARM64), ONNX Runtime can leverage
//! the Qualcomm QNN Execution Provider for Hexagon NPU acceleration.
//! On other platforms, it falls back to CPU execution.

use async_trait::async_trait;
use crate::{InferenceProvider, InferenceResponse, InferenceRequest, ChatRequest, InferenceStream, InferenceError, Result};

/// ONNX Runtime inference provider.
///
/// Wraps an ONNX model file. The model is loaded lazily on first use.
/// Requires the ONNX Runtime shared library to be installed on the system,
/// or downloaded at build time via the `ort` crate's `download` feature.
pub struct OnnxRuntimeProvider {
    model_path: String,
}

impl OnnxRuntimeProvider {
    pub fn new(model_path: impl Into<String>) -> Self {
        Self {
            model_path: model_path.into(),
        }
    }

    /// Returns the configured model path.
    pub fn model_path(&self) -> &str {
        &self.model_path
    }
}

#[async_trait]
impl InferenceProvider for OnnxRuntimeProvider {
    async fn complete(&self, _request: InferenceRequest) -> Result<InferenceResponse> {
        Err(InferenceError::Provider(
            "ONNX Runtime provider: direct completion not implemented. Use the OpenAI provider for text generation, or configure chat streaming.".to_string()
        ))
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f64>> {
        Err(InferenceError::Provider(
            "ONNX Runtime provider: embeddings not yet implemented".to_string()
        ))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        Ok(vec![format!("onnx:{}", self.model_path)])
    }

    fn default_model(&self) -> String {
        format!("onnx:{}", self.model_path)
    }

    async fn complete_chat_stream(&self, _request: ChatRequest) -> Result<InferenceStream> {
        Err(InferenceError::Provider(
            "ONNX Runtime provider: chat streaming not yet implemented. Use the OpenAI provider for interactive chat.".to_string()
        ))
    }
}
