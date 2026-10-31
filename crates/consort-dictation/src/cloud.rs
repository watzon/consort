//! What happens when Deepgram cannot write a part (design V4). The final pass
//! waits for the choice of the user. More parts wait behind it, and the preview
//! goes on, so the words stay on the screen and no audio is lost.

use crossbeam_channel::Receiver;
use sayso_sdk::deepgram::{self, Deepgram, DeepgramFinal};
use sayso_sdk::{EngineError, EngineEvent, ModelId, Segment, SpeechEngine, SpeechPart, Turn};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

/// Why Deepgram wrote nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudLoss {
    Connection,
    KeyRefused,
}

/// The answer of the user to a [`CloudLoss`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudChoice {
    TryAgain,
    FinishOnDevice,
    Cancel,
}

#[derive(Default)]
struct State {
    client: Option<Deepgram>,
    local_final: Option<String>,
    choice: Option<CloudChoice>,
    /// The user chose this device for the rest of this dictation.
    local: bool,
    cancelled: bool,
}

/// What the dictation and the layer share.
#[derive(Default)]
pub(crate) struct CloudGate {
    state: Mutex<State>,
    answered: Condvar,
}

impl CloudGate {
    fn lock(&self) -> MutexGuard<'_, State> {
        // The state stays valid when a thread panics: each field stands alone.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Readies the gate for one dictation.
    pub(crate) fn begin(&self, key: Option<(String, Option<String>)>, local_final: Option<String>) {
        let mut state = self.lock();
        state.client = key.map(|(key, url)| match url {
            Some(url) => Deepgram::with_base_url(key, url),
            None => Deepgram::new(key),
        });
        state.local_final = local_final;
        state.choice = None;
        state.local = false;
        state.cancelled = false;
    }

    pub(crate) fn has_local_final(&self) -> bool {
        self.lock().local_final.is_some()
    }

    pub(crate) fn answer(&self, choice: CloudChoice) {
        let mut state = self.lock();
        state.choice = Some(choice);
        if choice == CloudChoice::Cancel {
            state.cancelled = true;
        }
        self.answered.notify_all();
    }

    fn wait_for_answer(&self) -> CloudChoice {
        let mut state = self.lock();
        loop {
            if state.cancelled {
                return CloudChoice::Cancel;
            }
            if let Some(choice) = state.choice.take() {
                return choice;
            }
            state = self
                .answered
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Tells the dictation about a loss.
pub(crate) type LossFn = Arc<dyn Fn(CloudLoss) + Send + Sync>;

/// Sends a final pass of Deepgram to the client in the gate. It asks the user
/// when that fails. All other calls go to the engine.
pub(crate) struct CloudLayer {
    local: Arc<dyn SpeechEngine>,
    gate: Arc<CloudGate>,
    on_loss: LossFn,
    on_back: Arc<dyn Fn() + Send + Sync>,
}

impl CloudLayer {
    pub(crate) fn new(
        local: Arc<dyn SpeechEngine>,
        gate: Arc<CloudGate>,
        on_loss: LossFn,
        on_back: Arc<dyn Fn() + Send + Sync>,
    ) -> CloudLayer {
        CloudLayer {
            local,
            gate,
            on_loss,
            on_back,
        }
    }

    /// The final pass of one part with Deepgram, until it works or the user
    /// chooses something else.
    fn cloud_final(&self, samples: &[f32], language: &str) -> Result<Vec<Segment>, EngineError> {
        let mut lost = false;
        loop {
            let (client, local, cancelled) = {
                let state = self.gate.lock();
                (
                    state.client.clone(),
                    state.local.then(|| state.local_final.clone()).flatten(),
                    state.cancelled,
                )
            };
            if cancelled {
                return Err(EngineError::new("the dictation was cancelled"));
            }
            if let Some(model) = local {
                return self.local_final(&model, samples, language);
            }
            let Some(client) = client else {
                return Err(EngineError::new("there is no Deepgram key"));
            };
            let layer = DeepgramFinal::new(self.local.clone(), client);
            let model = ModelId::new(deepgram::MODEL);
            match layer.transcribe_segments(samples, &model, language) {
                Ok(segments) => {
                    if lost {
                        (self.on_back)();
                    }
                    return Ok(segments);
                }
                Err(error) => {
                    let Some(loss) = loss_of(&error) else {
                        return Err(error);
                    };
                    lost = true;
                    (self.on_loss)(loss);
                    match self.gate.wait_for_answer() {
                        CloudChoice::TryAgain => {}
                        CloudChoice::FinishOnDevice => self.gate.lock().local = true,
                        CloudChoice::Cancel => return Err(error),
                    }
                }
            }
        }
    }

    fn local_final(
        &self,
        model: &str,
        samples: &[f32],
        language: &str,
    ) -> Result<Vec<Segment>, EngineError> {
        let model = ModelId::new(model);
        self.local.load(&model)?;
        self.local.transcribe_segments(samples, &model, language)
    }
}

fn loss_of(error: &EngineError) -> Option<CloudLoss> {
    match error.code.as_ref().map(|code| code.as_str()) {
        Some(deepgram::CODE_CONNECTION) => Some(CloudLoss::Connection),
        Some(deepgram::CODE_KEY) => Some(CloudLoss::KeyRefused),
        _ => None,
    }
}

impl SpeechEngine for CloudLayer {
    fn load(&self, model: &ModelId) -> Result<(), EngineError> {
        if model.as_str() == deepgram::MODEL {
            Ok(())
        } else {
            self.local.load(model)
        }
    }

    fn transcribe(
        &self,
        samples: &[f32],
        model: &ModelId,
        language: &str,
    ) -> Result<String, EngineError> {
        if model.as_str() != deepgram::MODEL {
            return self.local.transcribe(samples, model, language);
        }
        let segments = self.cloud_final(samples, language)?;
        let texts: Vec<String> = segments.into_iter().map(|s| s.text).collect();
        Ok(texts.join(" "))
    }

    fn transcribe_segments(
        &self,
        samples: &[f32],
        model: &ModelId,
        language: &str,
    ) -> Result<Vec<Segment>, EngineError> {
        if model.as_str() != deepgram::MODEL {
            return self.local.transcribe_segments(samples, model, language);
        }
        self.cloud_final(samples, language)
    }

    fn detect_speech(
        &self,
        model: &ModelId,
        samples: &[f32],
    ) -> Result<Vec<SpeechPart>, EngineError> {
        self.local.detect_speech(model, samples)
    }

    fn diarize(
        &self,
        model: &ModelId,
        samples: &[f32],
        speakers: Option<u32>,
    ) -> Result<Vec<Turn>, EngineError> {
        self.local.diarize(model, samples, speakers)
    }

    fn start_stream(
        &self,
        session: u64,
        model: &ModelId,
        language: &str,
    ) -> Result<(), EngineError> {
        self.local.start_stream(session, model, language)
    }

    fn start_vad(&self, session: u64, model: &ModelId) -> Result<(), EngineError> {
        self.local.start_vad(session, model)
    }

    fn push_audio(&self, session: u64, samples: &[f32]) -> Result<(), EngineError> {
        self.local.push_audio(session, samples)
    }

    fn end_stream(&self, session: u64) {
        self.local.end_stream(session);
    }

    fn end_vad(&self, session: u64) -> Result<(), EngineError> {
        self.local.end_vad(session)
    }

    fn events(&self) -> Receiver<EngineEvent> {
        self.local.events()
    }
}
