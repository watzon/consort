//! Write in my voice against the fake hub: the settings, Try it, Edit on a
//! draft card and the tick. `fake_hub.rs` sends these request types here.

use super::*;
use consort_proto::model::{VoiceSample, VoiceSampleState, VoiceSettings};

/// The voice state of the fake hub.
#[derive(Default)]
pub struct State {
    pub settings: VoiceSettings,
}

/// The request types that this part answers.
pub fn handles(kind: &str) -> bool {
    matches!(
        kind,
        "voice.get"
            | "voice.set"
            | "voice.learn"
            | "voice.reset"
            | "voice.try"
            | "voice.feedback"
            | "approval.edit"
    )
}

/// Answers one request.
pub fn answer(hub: &Shared, req: &RequestFrame) -> Frame {
    let mut h = hub.lock().unwrap();
    h.recorded.push((req.kind.clone(), req.body.clone()));
    let result = |settings: &VoiceSettings, sample: Option<VoiceSample>| api::VoiceResult {
        settings: settings.clone(),
        sample,
        proposal: None,
    };
    match req.kind.as_str() {
        "voice.get" => req.respond_ok(&result(&h.voice.settings, None)).unwrap(),
        "voice.set" => {
            let body: api::VoiceSet = req.decode_as().unwrap();
            if let Some(enabled) = body.enabled {
                h.voice.settings.enabled = enabled;
            }
            if let Some(sources) = body.sources {
                h.voice.settings.sources = sources;
            }
            if let Some(disclosure) = body.disclosure {
                h.voice.settings.disclosure = disclosure;
            }
            req.respond_ok(&result(&h.voice.settings, None)).unwrap()
        }
        "voice.learn" | "voice.reset" => req.respond_ok(&result(&h.voice.settings, None)).unwrap(),
        "voice.try" => {
            let body: api::VoiceTry = req.decode_as().unwrap();
            h.voice.settings.sample_id += 1;
            h.voice.settings.sample_state = Some(VoiceSampleState::Ready);
            let sample = VoiceSample {
                id: h.voice.settings.sample_id,
                request: body.request,
                state: VoiceSampleState::Ready,
                draft: Some("Hi Dana,\nYes.\nChris".into()),
                context: Some("email.formal".into()),
            };
            req.respond_ok(&result(&h.voice.settings, Some(sample)))
                .unwrap()
        }
        "voice.feedback" => req.respond_ok(&Empty {}).unwrap(),
        "approval.edit" => {
            let body: api::ApprovalEdit = req.decode_as().unwrap();
            let mut approval = h.approval.clone().unwrap();
            if let ApprovalDetails::Connector(card) = &mut approval.details {
                card.draft_arguments = Some(card.arguments.clone());
                for (key, value) in &body.fields {
                    card.arguments[key.as_str()] = serde_json::Value::String(value.clone());
                }
                card.flags.clear();
            }
            h.approval = Some(approval.clone());
            req.respond_ok(&ApprovalResult { approval }).unwrap()
        }
        other => req.respond_error(ErrorBody::new(
            ErrorCode::Invalid,
            format!("The fake voice part has no {other}."),
        )),
    }
}
