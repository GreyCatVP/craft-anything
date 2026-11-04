use super::{chat_model, probe_local};

#[test]
fn gguf_paths_keep_specialised_model_directories() {
    for model in [
        "bge-small/ggml-model-f16.gguf",
        "bge-small/ggml-model.gguf",
        "bge-small/model.gguf",
        "models/bert-bge-small/ggml-model-f16.GGUF",
        r"models\bge-small\ggml-model-f16.gguf",
    ] {
        assert!(chat_model(&[model.into()]).is_none(), "{model}");
    }
}

#[test]
fn numeric_task_versions_are_not_chat_models() {
    for model in ["snowflake-arctic-embed2:latest", "vendor/embedding3", "vendor/rerank12", "XTTS2", "vendor/whisper3"] {
        assert!(chat_model(&[model.into()]).is_none(), "{model}");
    }
}

#[test]
fn publishers_and_non_task_words_still_chat() {
    for model in [
        "matts/llama-3-instruct",
        "embedding-author/qwen-2.5-instruct",
        "embed2/qwen-3",
        "whisper/gemma-3",
        "embedded-chat",
        "vendor/embedded2-chat",
        "vendor/embed2publisher-chat",
        "vendor/embed2text-chat",
        "vendor/bge2x-chat",
        "models/matts/llama-3.gguf",
        "models/embedded-chat/ggml-model-f16.gguf",
        "/profiles/whisper/models/qwen3.gguf",
        "models/embedding/qwen3.gguf",
    ] {
        assert_eq!(chat_model(&[model.into()]).as_deref(), Some(model), "{model}");
    }
}

#[test]
fn discovery_skips_specialised_paths_and_versions_before_chat() {
    let url = crate::tests::serve(
        "200 OK",
        r#"{"object":"list","data":[{"id":"bge-small/ggml-model-f16.gguf"},{"id":"snowflake-arctic-embed2:latest"},{"id":"matts/llama-3-instruct"}]}"#,
    );
    assert_eq!(probe_local("Local", &url).unwrap().suggestion.model, "matts/llama-3-instruct");
}

#[test]
fn discovery_does_not_offer_a_specialised_path_or_version_only_server() {
    let url = crate::tests::serve("200 OK", r#"{"object":"list","data":[{"id":"snowflake-arctic-embed2:latest"},{"id":"bge-small/ggml-model-f16.gguf"}]}"#);
    assert!(probe_local("Local", &url).is_none());
}
