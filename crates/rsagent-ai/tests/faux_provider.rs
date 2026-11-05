//! Integration tests for provider routing, driven by the offline `FauxProvider`.

use rsagent_ai::{Context, FauxProvider, FauxResponse, Model, Models, StopReason, StreamOptions};

/// A registry holding one shared faux provider.
fn registry(faux: &FauxProvider) -> Models {
    let mut models = Models::new();
    models.register(faux.clone());
    models
}

#[tokio::test]
async fn complete_returns_the_final_assistant_message() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![FauxResponse::text("the answer")]);
    let models = registry(&faux);

    let message = models
        .complete(&faux.model(), Context::default(), StreamOptions::default())
        .await;

    assert_eq!(message.to_text(), "the answer");
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(faux.call_count(), 1);
}

#[tokio::test]
async fn unknown_provider_surfaces_as_an_error_message() {
    let models = Models::new();
    let model = Model::openai_compatible("ghost", "ghost-1", "http://localhost");

    let message = models
        .complete(&model, Context::default(), StreamOptions::default())
        .await;

    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(message.error_message.is_some());
}

#[tokio::test]
async fn an_exhausted_queue_surfaces_as_an_error_message() {
    let faux = FauxProvider::new();
    let models = registry(&faux);

    let message = models
        .complete(&faux.model(), Context::default(), StreamOptions::default())
        .await;

    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(faux.pending_count(), 0);
}
