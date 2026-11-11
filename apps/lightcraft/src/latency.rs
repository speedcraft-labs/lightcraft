//! Attach the opt-in recorder to the window's actual egui command encoder.
use eframe::egui_wgpu::{self, wgpu};
use lightcraft_ui_egui::LightcraftApp;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

type Work = Arc<AtomicBool>;
static POLLER: OnceLock<mpsc::Sender<Work>> = OnceLock::new();

pub fn frame(app: &mut LightcraftApp, ui: &egui::Ui, frame: &eframe::Frame) {
    let Some(done) = app.latency_frame() else { return };
    let Some(rs) = frame.wgpu_render_state() else { return };
    let device = rs.device.clone();
    let poller = POLLER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Work>();
        let spawned = std::thread::Builder::new().name("latency-fence".into()).spawn(move || {
            while let Ok(done) = rx.recv() {
                while !done.load(Ordering::Acquire) {
                    // Independent of UI/control traffic. A timestamp is an upper bound on GPU
                    // completion by the observed poll/scheduling delay, never a scanout time.
                    if device.poll(wgpu::PollType::Poll).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(100));
                }
            }
        });
        if let Err(e) = spawned {
            log::error!("latency poller: {e}");
        }
        tx
    });
    ui.painter()
        .add(egui_wgpu::Callback::new_paint_callback(ui.max_rect(), Fence { done: std::sync::Mutex::new(Some(done)), poller: poller.clone() }));
}
struct Fence {
    done: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
    poller: mpsc::Sender<Work>,
}
impl egui_wgpu::CallbackTrait for Fence {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(callback) = self.done.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            let done = Arc::new(AtomicBool::new(false));
            let worker_done = done.clone();
            encoder.on_submitted_work_done(move || {
                callback();
                done.store(true, Ordering::Release);
            });
            let _ = self.poller.send(worker_done);
        }
        vec![]
    }
    fn paint(&self, _info: egui::PaintCallbackInfo, _pass: &mut wgpu::RenderPass<'static>, _resources: &egui_wgpu::CallbackResources) {}
}
