//! Opt-in UI-acceptance → reflecting-frame GPU completion observations.
//! No transport clocks are used. A host without a completion hook cannot finish a record.
use crate::{LightcraftApp, control, render::Slot};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc::Sender};
use std::time::Instant;

const MAX_RECORDS: usize = 4096;

#[derive(Default)]
struct State {
    records: Vec<Value>,
    waiters: Vec<(String, Sender<Value>)>,
}

/// A bounded, session-local recorder. Constructed only when explicitly enabled.
pub struct Recorder {
    epoch: Instant,
    state: Arc<Mutex<State>>,
    active: Option<Action>,
    frame: u64,
}
struct Action {
    id: String,
    kind: String,
    accepted: u64,
    frame: u64,
    initial_key: Option<u64>,
}

impl Recorder {
    pub fn from_env() -> Option<Self> {
        (std::env::var("LIGHTCRAFT_LATENCY").as_deref() == Ok("1")).then(Self::new)
    }
    /// Desktop entry-point clock, before settings, engine and window creation.
    pub fn startup(&mut self, entry: Instant) {
        self.epoch = entry;
        if self.begin(&json!({"id":"startup","action":"startup"}), None).is_ok() {
            if let Some(a) = self.active.as_mut() {
                a.accepted = 0;
            }
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(r) = state.records.last_mut() {
                r["accepted_ns"] = json!(0);
            }
        }
    }
    fn new() -> Self {
        Self { epoch: Instant::now(), state: Arc::new(Mutex::new(State::default())), active: None, frame: 0 }
    }
    pub(crate) fn begin(&mut self, meta: &Value, initial_key: Option<u64>) -> Result<(), &'static str> {
        let id = meta["id"].as_str().filter(|s| !s.is_empty() && s.len() <= 128).ok_or("invalid latency id")?;
        let kind = meta["action"]
            .as_str()
            .filter(|s| matches!(*s, "import" | "preview" | "exposure" | "switch" | "export" | "startup"))
            .ok_or("invalid latency action")?;
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.records.len() >= MAX_RECORDS {
            return Err("latency record capacity exceeded");
        }
        if self.active.as_ref().is_some_and(|a| !state.records.iter().any(|r| r["id"] == a.id && !r["completed_ns"].is_null())) {
            return Err("previous latency action incomplete");
        }
        if state.records.iter().any(|r| r["id"] == id) {
            return Err("duplicate latency id");
        }
        let accepted = ns(self.epoch);
        state.records.push(json!({"id":id,"action":kind,"accepted_ns":accepted,"first_reflecting_frame_presented_ns":null,"completed_ns":null,
            "endpoint":"reflecting-frame-gpu-work-confirmed-complete","control_wait_ns":0}));
        self.active = Some(Action { id: id.into(), kind: kind.into(), accepted, frame: self.frame, initial_key });
        Ok(())
    }
    pub(crate) fn wait(&mut self, params: &Value, reply: Sender<Value>) -> Result<(), &'static str> {
        let id = params["id"].as_str().ok_or("missing latency id")?;
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = state.records.iter().find(|r| r["id"] == id).ok_or("unknown latency id")?;
        if !record["completed_ns"].is_null() {
            let _ = reply.send(control::success(record.clone()));
        } else if state.waiters.len() < 16 {
            state.waiters.push((id.into(), reply));
        } else {
            return Err("too many latency waiters");
        }
        Ok(())
    }
    /// Called only by an enabled host after layout. The closure must be attached to THIS
    /// frame's submission, never a prior queue fence or the next event-loop callback.
    fn observe(&mut self, first: bool, complete: bool, pending: Vec<String>) -> Option<Box<dyn FnOnce() + Send>> {
        self.frame = self.frame.saturating_add(1);
        let a = self.active.as_ref()?;
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = state.records.iter().find(|r| r["id"] == a.id)?;
        let first = first && record["first_reflecting_frame_presented_ns"].is_null();
        let complete = complete && record["completed_ns"].is_null();
        drop(state);
        if !first && !complete {
            return None;
        }
        let (state, epoch, id, frames, accepted) = (self.state.clone(), self.epoch, a.id.clone(), self.frame.saturating_sub(a.frame), a.accepted);
        Some(Box::new(move || {
            // Timestamp BEFORE taking the lock or replying over the control channel.
            let end = ns(epoch);
            let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(r) = state.records.iter_mut().find(|r| r["id"] == id) else { return };
            if first && r["first_reflecting_frame_presented_ns"].is_null() {
                r["first_reflecting_frame_presented_ns"] = json!(end);
                r["first_response_ns"] = json!(end.saturating_sub(accepted));
                r["first_frames"] = json!(frames);
                r["first_background_pending"] = json!(pending);
            }
            if complete && r["completed_ns"].is_null() {
                r["completed_ns"] = json!(end);
                r["duration_ns"] = json!(end.saturating_sub(accepted));
                r["completion_frames"] = json!(frames);
                r["completion_background_pending"] = json!(pending);
            }
            let result = r.clone();
            if complete {
                state.waiters.retain(|(wait_id, reply)| {
                    if *wait_id == id {
                        let _ = reply.send(control::success(result.clone()));
                        false
                    } else {
                        true
                    }
                });
            }
        }))
    }
}
fn ns(epoch: Instant) -> u64 {
    u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

impl LightcraftApp {
    /// Pending work for admission, including quiet tasks and newly started activity rows.
    pub fn latency_pending(&self) -> Vec<String> {
        let mut p = self.tasks.labels();
        p.extend(self.session.activity.list().into_iter().map(|t| t.label));
        if self.renderer.queued() > 0 || self.renderer.in_flight() > 0 {
            p.push("renderer".into());
        }
        if self.scan.is_some() || self.import.is_some() {
            p.push("import".into());
        }
        if self.export.is_some() {
            p.push("export".into());
        }
        if self.merge.busy() {
            p.push("merge".into());
        }
        if !self.pending_gestures.is_empty() || !self.synthetic.is_empty() {
            p.push("input".into());
        }
        p
    }
    pub fn latency_frame(&mut self) -> Option<Box<dyn FnOnce() + Send>> {
        // Critical: do not even inspect app state on the disabled path.
        let recorder = self.latency.as_ref()?;
        let a = recorder.active.as_ref();
        let pending = self.latency_pending();
        let key = self.renderer.textures.get(&Slot::Main).map(|t| t.key);
        let main_ready = self.loupe_shown.is_some_and(|(photo, src)| matches!(src, "render" | "cached") && self.session.active() == Some(photo))
            && !self.renderer.is_pending(Slot::Main)
            && !self.renderer.is_pending(Slot::Region);
        let (first, complete) = match a.map(|a| (a.kind.as_str(), a.initial_key)) {
            Some(("startup", _)) => (
                main_ready || self.renderer.thumb_textures() > 0 || (self.session.active().is_none() && self.fonts_ready),
                main_ready || self.renderer.thumb_textures() > 0 || (self.session.active().is_none() && self.fonts_ready),
            ),
            Some(("import", _)) => {
                // The grid is virtualized: only on-screen cells can be presented in a frame.
                // The driver verifies every catalog filename and every page outside this clock.
                let grid = self.widgets.iter().rev().find(|(id, _)| id == "grid:scroll").map(|(_, rect)| *rect);
                let visible: Vec<_> = self
                    .widgets
                    .iter()
                    .filter_map(|(id, rect)| {
                        let photo = id.strip_prefix("thumb:")?.parse::<u64>().ok()?;
                        grid.filter(|g| g.intersects(*rect)).map(|_| lightcraft_catalog::PhotoId(photo))
                    })
                    .collect();
                let shown =
                    |photo| self.renderer.textures.contains_key(&Slot::Thumb(photo)) || self.renderer.textures.contains_key(&Slot::ThumbQuick(photo));
                (
                    !visible.is_empty() && visible.iter().copied().any(shown),
                    self.import.is_none() && self.scan.is_none() && !visible.is_empty() && visible.iter().copied().all(shown),
                )
            }
            Some(("export", _)) => {
                (self.export.is_none() && self.last_export_result.is_some(), self.export.is_none() && self.last_export_result.is_some())
            }
            Some(("exposure", old)) => (key != old && main_ready, key != old && main_ready),
            Some(("preview" | "switch", _)) => {
                (self.loupe_shown.is_some_and(|(photo, src)| src != "none" && self.session.active() == Some(photo)), main_ready)
            }
            _ => (false, false),
        };
        self.latency.as_mut()?.observe(first, complete, pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_only_complete_on_the_hosts_frame_fence() {
        let mut r = Recorder::new();
        r.begin(&json!({"id":"1","action":"switch"}), None).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        r.wait(&json!({"id":"1"}), tx).unwrap();
        assert!(r.observe(false, false, vec![]).is_none());
        let done = r.observe(true, true, vec!["prefetch".into()]).unwrap();
        assert!(rx.try_recv().is_err());
        done();
        let value = rx.recv().unwrap();
        assert!(value["result"]["duration_ns"].as_u64().unwrap() > 0);
        assert_eq!(value["result"]["control_wait_ns"], 0);
        assert_eq!(value["result"]["completion_frames"], 2);
        assert!(r.begin(&json!({"id":"1","action":"switch"}), None).is_err());
    }
    #[test]
    fn disabled_recorder_adds_no_frame_work() {
        let mut app = crate::headless::Headless::new(
            crate::LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default()),
            [800.0, 600.0],
            1.0,
        );
        app.app.latency = None;
        let before = app.app.renderer.completed;
        assert!(app.app.latency_frame().is_none());
        assert_eq!(app.app.renderer.completed, before);
    }
}
