// The Windows speech recognizer (WinRT). The microphone is opened only by `Session::start`'s
// StartAsync and closed by its stop; `probe` never starts a session.

use serde_json::{json, Value};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use windows::core::HSTRING;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::SpeechRecognition::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Windows' "online speech recognition is off" (SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED).
const PRIVACY_OFF: i32 = 0x80045509u32 as i32;

pub struct SrErr {
    pub code: i32,
    pub msg: String,
}

impl From<windows::core::Error> for SrErr {
    fn from(e: windows::core::Error) -> Self {
        SrErr { code: e.code().0, msg: e.message() }
    }
}

impl SrErr {
    pub fn json(&self) -> Value {
        json!({ "code": format!("0x{:08X}", self.code as u32), "privacy": self.code == PRIVACY_OFF, "message": self.msg })
    }
}

fn com() {
    // Already initialised on this thread is fine (returns S_FALSE / RPC_E_CHANGED_MODE).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn build() -> Result<SpeechRecognizer, SrErr> {
    let r = SpeechRecognizer::new()?;
    let topic = SpeechRecognitionTopicConstraint::Create(SpeechRecognitionScenario::Dictation, &HSTRING::new())?;
    r.Constraints()?.Append(&topic)?;
    let res = r.CompileConstraintsAsync()?.get()?;
    if res.Status()? != SpeechRecognitionResultStatus::Success {
        return Err(SrErr { code: 0, msg: format!("constraints not compiled: {:?}", res.Status()?) });
    }
    Ok(r)
}

/// Creates the recognizer and compiles its constraints; records nothing.
pub fn probe() -> Result<Value, SrErr> {
    com();
    let r = SpeechRecognizer::new()?;
    let lang = r.CurrentLanguage()?;
    let mut out = json!({ "language": lang.LanguageTag()?.to_string(), "name": lang.DisplayName()?.to_string() });
    let sup: Vec<String> = SpeechRecognizer::SupportedTopicLanguages()?.into_iter().filter_map(|l| l.LanguageTag().ok()).map(|s| s.to_string()).collect();
    out["supported"] = json!(sup);
    let topic = SpeechRecognitionTopicConstraint::Create(SpeechRecognitionScenario::Dictation, &HSTRING::new())?;
    r.Constraints()?.Append(&topic)?;
    let res = r.CompileConstraintsAsync()?.get()?;
    out["compile"] = json!(format!("{:?}", res.Status()?));
    let _ = r.Close();
    Ok(out)
}

pub struct Session {
    stop: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl Session {
    pub fn start(emit: Emit) -> Session {
        let (tx, rx) = mpsc::channel::<()>();
        let tx2 = tx.clone();
        let join = std::thread::spawn(move || {
            com();
            if let Err(e) = run(&emit, tx2, rx) {
                emit("error", e.json());
            }
            emit("ended", json!({}));
        });
        Session { stop: tx, join: Some(join) }
    }

    pub fn stop(mut self) {
        let _ = self.stop.send(());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn run(emit: &Emit, tx: mpsc::Sender<()>, rx: mpsc::Receiver<()>) -> Result<(), SrErr> {
    let r = build()?;
    let sess = r.ContinuousRecognitionSession()?;
    // ponytail: 30 s of silence ends the session (the default is 5 s); the page shows it as stopped.
    sess.SetAutoStopSilenceTimeout(TimeSpan { Duration: 30 * 10_000_000 })?;
    let e1 = emit.clone();
    let t1 = sess.ResultGenerated(&TypedEventHandler::new(move |_, args: windows::core::Ref<'_, SpeechContinuousRecognitionResultGeneratedEventArgs>| {
        if let Some(a) = args.as_ref() {
            let res = a.Result()?;
            if res.Confidence()? != SpeechRecognitionConfidence::Rejected {
                let text = res.Text()?.to_string();
                if !text.trim().is_empty() {
                    e1("text", json!({ "text": text }));
                }
            }
        }
        Ok(())
    }))?;
    let t2 = sess.Completed(&TypedEventHandler::new(move |_, _| {
        let _ = tx.send(());
        Ok(())
    }))?;
    let started = sess.StartAsync().and_then(|a| a.get());
    if let Err(e) = started {
        let _ = sess.RemoveResultGenerated(t1);
        let _ = sess.RemoveCompleted(t2);
        let _ = r.Close();
        return Err(e.into());
    }
    emit("listening", json!({}));
    let _ = rx.recv();
    if let Ok(a) = sess.StopAsync() {
        let _ = a.get();
    }
    let _ = sess.RemoveResultGenerated(t1);
    let _ = sess.RemoveCompleted(t2);
    let _ = r.Close();
    Ok(())
}
