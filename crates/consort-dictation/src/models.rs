//! Which models a dictation needs (plan, decisions 4, 5 and 8).

use crate::settings::{Engine, Settings};
use sayso_sdk::cleanup;
use sayso_sdk::deepgram;
use sayso_sdk::embedded::{ModelInfo, Role, VadRun};
use std::path::Path;

/// The final model of Consort. It hears 25 European languages.
const FINAL_MODEL: &str = "parakeet-tdt-v3";
/// The preview in English on an engine where the final model has no preview.
const PREVIEW_ENGLISH: &str = "parakeet-eou-120m";
/// The preview in other languages on such an engine.
const PREVIEW_OTHER: &str = "nemotron-multilingual";
const VAD_MODEL: &str = "silero-vad";

/// What a model does in a dictation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Final,
    Preview,
    Vad,
    Cleanup,
}

/// A model that a dictation uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelNeed {
    pub id: String,
    /// The full name. S1-mini always shows with "Superwhisper".
    pub name: String,
    pub size_bytes: u64,
    pub purpose: Purpose,
    pub on_disk: bool,
}

impl ModelNeed {
    /// The line of the download notice: "Parakeet TDT v3, final text". The
    /// voice detection model is about 1 MB, and it has no line of its own.
    pub fn line(&self) -> Option<String> {
        let purpose = match self.purpose {
            Purpose::Final => "final text",
            Purpose::Preview => "live preview",
            Purpose::Cleanup => "cleanup",
            Purpose::Vad => return None,
        };
        Some(format!("{}, {purpose}", self.name))
    }
}

/// The models of a dictation with these settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPlan {
    pub final_model: String,
    /// The local model of "Finish on this device" with Deepgram.
    pub local_final: Option<String>,
    pub preview: Option<String>,
    pub vad: VadRun,
    /// All models with their state, also the ones on disk.
    pub needs: Vec<ModelNeed>,
}

impl ModelPlan {
    /// The models to download first.
    pub fn missing(&self) -> Vec<ModelNeed> {
        self.needs.iter().filter(|n| !n.on_disk).cloned().collect()
    }
}

/// Chooses from the catalog of the engine. The catalog decides, so this needs
/// no platform checks: where the final model has the streaming role (the Rust
/// engine), it is the preview too.
pub fn plan_models(settings: &Settings, catalog: &[ModelInfo], cleanup_dir: &Path) -> ModelPlan {
    let find = |id: &str| catalog.iter().find(|m| m.id == id);
    let mut needs = Vec::new();
    let mut need = |model: Option<&ModelInfo>, purpose| {
        if let Some(model) = model
            && !needs.iter().any(|n: &ModelNeed| n.id == model.id)
        {
            needs.push(ModelNeed {
                id: model.id.clone(),
                name: model.name.clone(),
                size_bytes: model.size_bytes,
                purpose,
                on_disk: model.on_disk(),
            });
        }
    };

    let local_final = find(FINAL_MODEL);
    let preview = match local_final {
        Some(model) if model.has_role(&Role::Streaming) => Some(model),
        _ if is_english(&settings.language) => find(PREVIEW_ENGLISH),
        _ => find(PREVIEW_OTHER),
    };
    let vad = find(VAD_MODEL);
    let deepgram = settings.engine == Engine::Deepgram;
    if !deepgram {
        need(local_final, Purpose::Final);
    }
    need(preview, Purpose::Preview);
    need(vad, Purpose::Vad);
    if settings.cleanup_on
        && let Some(model) = cleanup::find(&settings.cleanup_model)
    {
        needs.push(ModelNeed {
            id: model.id.to_owned(),
            name: cleanup_name(model),
            size_bytes: model.size_bytes,
            purpose: Purpose::Cleanup,
            on_disk: cleanup::download::is_ready(cleanup_dir, model),
        });
    }
    ModelPlan {
        final_model: if deepgram {
            deepgram::MODEL.to_owned()
        } else {
            FINAL_MODEL.to_owned()
        },
        local_final: local_final.filter(|m| m.on_disk()).map(|m| m.id.clone()),
        preview: preview.map(|m| m.id.clone()),
        vad: match vad {
            Some(model) if model.on_disk() => {
                VadRun::Model(sayso_sdk::embedded::ModelId::new(&model.id))
            }
            // The silence limit also works with the basic detector.
            _ => VadRun::Basic,
        },
        needs,
    }
}

fn is_english(language: &str) -> bool {
    language == "en" || language.starts_with("en-")
}

/// "Cleanup 1 by Spokenly" or "S1-mini by Superwhisper": the names are always
/// written in full.
pub fn cleanup_name(model: &cleanup::CleanupModelInfo) -> String {
    format!("{} by {}", model.name, model.publisher)
}

/// The cleanup models to choose from: the id and the text of the choice.
pub fn cleanup_choices() -> Vec<(String, String)> {
    cleanup::catalog()
        .iter()
        .rev()
        .map(|m| {
            (
                m.id.to_owned(),
                format!("{}, {}", cleanup_name(m), format_size(m.size_bytes)),
            )
        })
        .collect()
}

/// The languages of the final model, or the default when the engine does not run.
pub fn speech_languages(catalog: &[ModelInfo]) -> Vec<String> {
    catalog
        .iter()
        .find(|m| m.id == FINAL_MODEL)
        .map(|m| m.languages.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| vec!["en".to_owned()])
}

/// A size as the design writes it: 483 MB, 1.3 GB.
pub fn format_size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    let mb = bytes as f64 / MB;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{} MB", mb.round() as u64)
    }
}
