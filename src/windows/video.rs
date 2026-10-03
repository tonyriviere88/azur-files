//! The Media Engine in **frame-server mode**: Windows decodes and plays the file, and this asks
//! it for the frame that should be on screen right now.
//!
//! The Windows half of [`super`], and the only file in this program that talks to Media
//! Foundation. Everything above it — when a player is opened, how big a frame to ask for, what
//! the strip under the canvas says — is portable and lives next door.
//!
//! # Why the Media Engine and not a decoder
//!
//! The alternative inside Media Foundation is `IMFSourceReader`, which hands back decoded frames
//! and nothing else: no clock, no audio, no frame dropping, no A/V sync. Every one of those would
//! then be written here, and the audio one is not a small thing — a decoder with no output device
//! is a silent film. The Media Engine is the whole player: it resolves the file, picks the
//! hardware decoder, opens the audio device, keeps the two streams together and answers
//! `SetCurrentTime`. What it does *not* do, in this mode, is own a window.
//!
//! # Frame-server mode is chosen by omission
//!
//! Set `MF_MEDIA_ENGINE_PLAYBACK_HWND` and the engine renders into a window of its own; set
//! `MF_MEDIA_ENGINE_PLAYBACK_VISUAL` and it renders into a composition visual. **Setting neither
//! is what makes it a frame server**, which is the only one of the three this program can use:
//! the other two put a surface the compositor owns over the top of everything egui paints, which
//! is the airspace problem [`crate::preview::visual`]'s header sets out at length and the reason
//! `IPreviewHandler` was turned down there.
//!
//! So the deal is: the engine writes each frame into a texture this code owns, and this code
//! copies it back to the CPU and hands it to egui as an ordinary image. One readback per frame is
//! the price of not having a second compositor in the window, and it is bounded — see
//! [`super::Player::wanted`], which asks for the frame at *panel* size rather than the file's, so
//! a 4K film costs the same as a phone clip.
//!
//! # What is deliberately not here
//!
//! **No zero-copy into wgpu.** The shared-handle route — an `NT` handle out of D3D11, imported
//! into wgpu's Vulkan device through `create_texture_from_hal` — would save the readback and cost
//! the `AZUR_GLOW=1` escape hatch, since it can only work on one backend. An egui texture works
//! on both, which is the trade `azur_egui_theme::render` has already made everywhere else.

use std::cell::Cell;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::OnceLock;

use egui::{Color32, ColorImage};
use windows::core::{implement, Interface, BSTR, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MFCreateAttributes,
    MFCreateDXGIDeviceManager, MFStartup, CLSID_MFMediaEngineClassFactory,
    MFSTARTUP_NOSOCKET, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER,
    MF_MEDIA_ENGINE_ERR_DECODE, MF_MEDIA_ENGINE_ERR_ENCRYPTED, MF_MEDIA_ENGINE_ERR_NETWORK,
    MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED, MF_MEDIA_ENGINE_EVENT_ENDED,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA,
    MF_MEDIA_ENGINE_EVENT_PAUSE, MF_MEDIA_ENGINE_EVENT_PLAY, MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC,
    MF_MEDIA_ENGINE_READY_HAVE_CURRENT_DATA, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::UrlCreateFromPathW;

use super::Word;

/// Media Foundation, started once and never stopped.
///
/// The matching `MFShutdown` is skipped for the reason [`crate::preview::visual`] skips
/// `CoUninitialize`: the *last* one in a process tears down state other threads are still using,
/// and there is nothing to be gained by racing the process's own rundown for it. What it buys is
/// that a player opened, closed and opened again pays the startup once.
fn started() -> bool {
    static ONCE: OnceLock<bool> = OnceLock::new();
    *ONCE.get_or_init(|| {
        // SAFETY: the version constant is the one this header set was generated from, and
        // `NOSOCKET` asks for everything but the network sink — nothing here serves media.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.is_ok()
    })
}

/// The graphics device the engine decodes into, shared by every player on this thread.
///
/// **One device, not one per player.** Creating one costs several milliseconds and a few
/// megabytes, which is a visible hitch to pay on the frame the keyboard lands on a clip — and two
/// panes each showing a video is a case this panel exists for, so "a handful of players at once"
/// is the shape to plan for.
///
/// A `thread_local` rather than a `static`, because a COM interface is not `Send` and the honest
/// way to say "these are only ever touched on the UI thread" is to put them somewhere only one
/// thread can reach. Every player is created, ticked and dropped from `App::frame`.
struct Shared {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    manager: IMFDXGIDeviceManager,
}

thread_local! {
    static SHARED: Cell<Option<&'static Shared>> = const { Cell::new(None) };
}

/// The device for this thread, made once and **never given back**.
///
/// # Why it is leaked, which is the whole of this function
///
/// Releasing a D3D11 device that Media Foundation has been decoding on **hangs**, and it hangs
/// where nothing can be done about it. Measured, with a debugger on the stuck process:
///
/// ```text
/// drop_glue<Shared>  →  IMFDXGIDeviceManager::Release
///   →  d3d11!CDevice::LLOBeginLayerDestruction  →  nvwgf2umx  →  WaitForSingleObject   ⟵ forever
/// ```
///
/// Media Foundation is never shut down here — see [`started`], which has the same argument the shell
/// modules make about `CoUninitialize` — so its work queues are still live when the last reference to
/// the device goes, and the display driver waits for something that is never going to happen.
///
/// This was found because it is a `thread_local`: a thread-local destructor runs when its **thread**
/// exits, so a test thread that had previewed one video hung on the way out. The same destructor runs
/// on the main thread as the program closes, so what the leak buys is not tidiness in a test — it is
/// the window shutting instead of hanging with no window on screen and nothing to click.
///
/// So the device lives as long as the process, which is what it would have wanted anyway: it is the
/// graphics device, every player shares it, and the memory is a few megabytes that only exists at all
/// on a thread that has played something. Leaked memory is never released, so there is no destructor
/// to deadlock — and at process exit the kernel takes it back the way it takes everything else.
fn shared() -> Result<&'static Shared, String> {
    SHARED.with(|slot| {
        if let Some(shared) = slot.get() {
            return Ok(shared);
        }
        let made: &'static Shared = Box::leak(Box::new(make_device()?));
        slot.set(Some(made));
        Ok(made)
    })
}

/// The one complaint for every way the graphics device can fail to be made.
///
/// The same argument [`trouble`] makes about the engine: a device that would not be created, a
/// manager that would not be made and a device the manager would not take are one piece of news to
/// whoever is looking at the panel — this machine will not play videos here.
fn no_device() -> String {
    "No graphics device for video".to_owned()
}

/// The D3D11 device, its context, and the manager that lends it to Media Foundation.
fn make_device() -> Result<Shared, String> {
    // **An apartment on this thread**, because the Media Engine's class factory is reached through
    // `CoCreateInstance` and a thread with no apartment gets `CO_E_NOTINITIALIZED` instead of a
    // player. `main` does this for the window's own thread — but a test drives the application
    // without going through `main`, and every test thread is its own thread. Here rather than in
    // [`Engine::open`] so it happens once per thread and not once per file: `OleInitialize` counts
    // its calls, and this program deliberately never gives one back — see [`crate::shell::flush`].
    crate::shell::init();

    let mut device = None;
    let mut context = None;
    // SAFETY: both out-parameters are `Option`s the call fills in, and both are checked below.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            // The software rasteriser's module, which is only read for `D3D_DRIVER_TYPE_SOFTWARE`.
            HMODULE::default(),
            // `VIDEO_SUPPORT` is what makes hardware decoding reachable at all, and `BGRA_SUPPORT`
            // is what lets the engine write the one format worth asking for — see `ensure`.
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|_| no_device())?;
    }
    let (device, context) = match (device, context) {
        (Some(device), Some(context)) => (device, context),
        _ => return Err(no_device()),
    };

    // **The engine decodes on its own threads and this program transfers on the UI thread**, which
    // is two threads on one device — so the device has to be told. Without this the first
    // `TransferVideoFrame` under load corrupts a frame or takes the process out, and it does it
    // rarely enough to look like a driver bug.
    // SAFETY: the cast fails on a device that does not implement it, which is handled.
    unsafe {
        if let Ok(threading) = device.cast::<ID3D11Multithread>() {
            // The answer is whether it was *already* on, which is not a question anybody here has.
            let _ = threading.SetMultithreadProtected(true);
        }
    }

    let mut token = 0u32;
    let mut manager = None;
    // SAFETY: both out-parameters are written before the call returns success.
    let manager = unsafe {
        MFCreateDXGIDeviceManager(&mut token, &mut manager)
            .ok()
            .and(manager)
            .ok_or_else(no_device)?
    };
    // SAFETY: the device outlives the manager — both are held in the `Shared` this returns.
    unsafe {
        manager.ResetDevice(&device, token).map_err(|_| no_device())?;
    }

    Ok(Shared {
        device,
        context,
        manager,
    })
}

/// The engine's side of the conversation, which arrives on Media Foundation's threads.
///
/// **Nothing is done here but a `send` and a repaint**, and that is a rule rather than a
/// simplification: this runs on a worker inside the engine, where the application is not
/// reachable and a slow callback is a stalled decoder. It is the same shape as
/// [`crate::shell::dnd`]'s target, for the same reason.
#[implement(IMFMediaEngineNotify)]
struct Notify {
    /// **Behind a lock**, because a `Sender` is `Send` and not `Sync` and this is a COM object whose
    /// method takes `&self`. The engine delivers its events one at a time in practice, which is
    /// exactly the kind of fact that makes a data race a bug found in a year rather than a bug found
    /// in a day — and the lock is uncontended, so it costs an atomic.
    words: std::sync::Mutex<Sender<Word>>,
    /// How the frame loop is woken. A player that only spoke when something else asked for a
    /// frame would be a player that never started, because this program is idle between events.
    ctx: egui::Context,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> windows::core::Result<()> {
        // Compared as numbers rather than matched as patterns: these are newtypes over `i32` from
        // a generated header, and `event` arrives as a bare `u32`.
        let is = |what: windows::Win32::Media::MediaFoundation::MF_MEDIA_ENGINE_EVENT| {
            event == what.0 as u32
        };
        let word = if is(MF_MEDIA_ENGINE_EVENT_ERROR) {
            Some(Word::Failed(complaint(param1, param2)))
        } else if is(MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA) {
            Some(Word::Ready)
        } else if is(MF_MEDIA_ENGINE_EVENT_PLAY) {
            Some(Word::Playing)
        } else if is(MF_MEDIA_ENGINE_EVENT_PAUSE) {
            Some(Word::Paused)
        } else if is(MF_MEDIA_ENGINE_EVENT_ENDED) {
            Some(Word::Ended)
        } else {
            // `TIMEUPDATE`, `SEEKING`, `SEEKED`, the buffering events: the panel reads the clock
            // off the engine on every frame it draws, so an event saying it moved is an event
            // saying what is already known.
            None
        };
        if let Some(word) = word {
            // A poisoned lock means a previous call panicked inside it, which nothing here can do —
            // and a panic unwinding out of a COM method is worse than a dropped event.
            let sent = self
                .words
                .lock()
                .map(|words| words.send(word).is_ok())
                .unwrap_or(false);
            if sent {
                self.ctx.request_repaint();
            }
        }
        Ok(())
    }
}

/// Why the engine gave up, as a sentence for the middle of the panel.
///
/// The `HRESULT` is deliberately not shown. `MF_E_UNSUPPORTED_BYTESTREAM_TYPE` is not something to
/// put in front of somebody who selected a file — what they need to know is whether the file is
/// broken or the machine is missing a codec, and these five words are the whole of that answer.
fn complaint(err: usize, _hr: u32) -> String {
    let err = err as i32;
    if err == MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED.0 {
        // The common one, and the one worth being specific about: an AV1 or HEVC file on a machine
        // without that decoder installed lands here, and it is a fact about the machine rather
        // than about the file.
        "No codec for this video".to_owned()
    } else if err == MF_MEDIA_ENGINE_ERR_DECODE.0 {
        "This video cannot be decoded".to_owned()
    } else if err == MF_MEDIA_ENGINE_ERR_ENCRYPTED.0 {
        "This video is protected".to_owned()
    } else if err == MF_MEDIA_ENGINE_ERR_NETWORK.0 {
        "This video cannot be read".to_owned()
    } else {
        "This video cannot be played".to_owned()
    }
}

/// The two textures a frame passes through on its way to egui.
struct Surface {
    /// What the engine writes into. A render target, which is what `TransferVideoFrame` requires.
    target: ID3D11Texture2D,
    /// And what the processor can read: the same pixels, in memory it can map.
    staging: ID3D11Texture2D,
    size: [u32; 2],
}

/// One open file, playing.
pub(crate) struct Engine {
    engine: IMFMediaEngine,
    /// The device every player on this thread decodes on. Borrowed for the process's lifetime rather
    /// than counted, and [`shared`] is where the reason for that is written down at length.
    shared: &'static Shared,
    surface: Option<Surface>,
    words: Receiver<Word>,
    /// The presentation time of the last frame transferred.
    ///
    /// **This is how a new frame is told from the same one again**, and it has to be, because the
    /// obvious route does not work: `OnVideoStreamTick` answers `S_FALSE` when nothing has changed,
    /// `S_FALSE` is a success code, and the generated wrapper maps every success to `Ok` — so the
    /// `Result` says "fine" either way and only the timestamp inside it differs.
    last: i64,
}

impl Engine {
    /// Open a file and start playing it.
    ///
    /// Nothing here blocks on the file. `SetSource` hands the path to Media Foundation's resolver,
    /// which opens it on a worker and reports back through [`Notify`] — which is the whole reason
    /// this runs on the UI thread at all. The alternative, `MFCreateFile` and
    /// `SetSourceFromByteStream`, opens the file on the calling thread, and a file on a share that
    /// has gone away takes twenty seconds to fail: an unreachable path would freeze the window.
    pub(crate) fn open(path: &std::path::Path, ctx: &egui::Context) -> Result<Self, String> {
        if !started() {
            return Err("Media Foundation is not available".to_owned());
        }
        let shared = shared()?;
        // Before the engine, so a path that will not convert costs nothing to find out about — and so
        // there is one fewer way out of this function with a live engine in a local. See below.
        let url = url_for(path)?;
        let (sender, words) = channel();
        let notify: IMFMediaEngineNotify = Notify {
            words: std::sync::Mutex::new(sender),
            ctx: ctx.clone(),
        }
        .into();

        // SAFETY: every out-parameter is checked, and each interface is released by its own `Drop`.
        // The attribute store outlives `CreateInstance`, which is the only call that reads it.
        let engine = unsafe {
            // Three, which is how many are set below — the count is the store's initial room and
            // nothing breaks if it is wrong, which is exactly why it is worth keeping honest.
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 3).map_err(|_| trouble())?;
            let attributes = attributes.ok_or_else(trouble)?;
            attributes
                .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
                .map_err(|_| trouble())?;
            attributes
                .SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &shared.manager)
                .map_err(|_| trouble())?;
            // **The one format to ask for.** `TransferVideoFrame` will convert into whatever this
            // says, on the graphics device, which is where a colour conversion belongs — the
            // alternative is receiving NV12 and writing a YUV-to-RGB pass on the processor.
            attributes
                .SetUINT32(
                    &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                    DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
                )
                .map_err(|_| trouble())?;

            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|_| trouble())?;
            factory
                .CreateInstance(0, &attributes)
                .map_err(|_| trouble())?
        };

        // SAFETY: the engine was just created and the URL outlives the call, which copies it.
        unsafe {
            // **It does not start playing.** Arrowing onto a clip should show you the clip, not begin
            // a performance — the panel follows the keyboard, so autoplay would mean a folder of
            // videos playing one after another as you looked down it.
            //
            // Which makes the pair of settings below the interesting part: with autoplay off the
            // engine has no reason to fetch anything, so `PRELOAD_AUTOMATIC` is what makes it load
            // and decode as far as the first frame anyway. That is what there is to *show* — the
            // ready state reaching `HAVE_CURRENT_DATA` is exactly "there is a frame now", which is
            // what [`Engine::frame`] waits for. Without the preload the panel would sit on
            // `Opening…` until somebody pressed play, which is not a preview.
            //
            // Both before the source, which is what makes them take effect.
            let _ = engine.SetAutoPlay(false);
            let _ = engine.SetPreload(MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC);
            if engine.SetSource(&url).is_err() {
                // **Shut down on the way out.** Everything below this point is [`Drop`]'s job, but
                // the engine is not in a `Self` yet — so this one path has to do by hand what every
                // other path gets for free, and the reason is the same one `Drop` gives: an engine
                // merely released keeps the audio device.
                let _ = engine.Shutdown();
                return Err("This video cannot be opened".to_owned());
            }
        }

        Ok(Self {
            engine,
            shared,
            surface: None,
            words,
            last: i64::MIN,
        })
    }

    /// Everything the engine has said since the last look.
    pub(crate) fn words(&self) -> impl Iterator<Item = Word> + '_ {
        self.words.try_iter()
    }

    /// The file's own pixel size, once it is known. `None` before the metadata has landed.
    pub(crate) fn native(&self) -> Option<[u32; 2]> {
        let (mut w, mut h) = (0u32, 0u32);
        // SAFETY: two `u32`s on this frame's stack, written or left alone.
        unsafe { self.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h)) }.ok()?;
        (w > 0 && h > 0).then_some([w, h])
    }

    /// How long it runs, in seconds. `NaN` until the metadata has landed, and infinite for a
    /// stream — both of which [`super::Player`] treats as "no scrubber".
    pub(crate) fn duration(&self) -> f64 {
        // SAFETY: a plain getter on a live engine.
        unsafe { self.engine.GetDuration() }
    }

    /// Where it has got to, in seconds.
    pub(crate) fn at(&self) -> f64 {
        // SAFETY: as above.
        unsafe { self.engine.GetCurrentTime() }
    }

    pub(crate) fn paused(&self) -> bool {
        // SAFETY: as above.
        unsafe { self.engine.IsPaused() }.as_bool()
    }

    pub(crate) fn play(&self) {
        // SAFETY: as above. A failure means the engine is not in a state to play, which the next
        // frame reads back off `paused`.
        let _ = unsafe { self.engine.Play() };
    }

    pub(crate) fn pause(&self) {
        // SAFETY: as above.
        let _ = unsafe { self.engine.Pause() };
    }

    /// Go to `to` seconds.
    ///
    /// A seek while paused still produces a frame, which is what makes scrubbing a paused video
    /// show anything: the engine decodes the target frame and `OnVideoStreamTick` reports it with
    /// a new timestamp.
    pub(crate) fn seek(&self, to: f64) {
        // SAFETY: as above.
        let _ = unsafe { self.engine.SetCurrentTime(to.max(0.0)) };
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        // SAFETY: as above.
        let _ = unsafe { self.engine.SetMuted(muted) };
    }

    /// Whether the file has a sound track at all, so a mute button is only offered where it would
    /// do something.
    pub(crate) fn has_audio(&self) -> bool {
        // SAFETY: as above.
        unsafe { self.engine.HasAudio() }.as_bool()
    }

    /// The frame on screen now, at `size` pixels — or `None` when it is the one already held.
    ///
    /// **Asked for at the size it will be drawn**, which is the bound that makes the readback
    /// affordable: the engine scales on the graphics device, so a 4K film costs the same here as a
    /// phone clip. See [`super::Player::wanted`].
    pub(crate) fn frame(&mut self, size: [u32; 2]) -> Option<ColorImage> {
        let resized = self.ensure(size)?;
        // A texture that has just been made holds nothing, so the frame has to be transferred into
        // it even though the engine has not moved on. Forcing the timestamp is how: without it, a
        // paused video would go black the moment the panel was resized.
        if resized {
            self.last = i64::MIN;
        }
        let surface = self.surface.as_ref()?;

        // SAFETY: the engine and both textures are live for the whole block. `pts` is compared
        // rather than trusted as a signal — see the field.
        unsafe {
            // **There has to be a current frame before one is asked for.** `OnVideoStreamTick` says
            // `S_FALSE` when there is not, which arrives here as a success with an untouched
            // timestamp — and an untouched timestamp is zero, which differs from the impossible value
            // `last` starts at, so the first tick would transfer a frame that does not exist yet and
            // put a black rectangle where `Opening…` belongs. The ready state is the honest question:
            // `HAVE_METADATA` is what the panel learns the picture's size from, and this is the next
            // rung up.
            if self.engine.GetReadyState() < MF_MEDIA_ENGINE_READY_HAVE_CURRENT_DATA.0 as u16 {
                return None;
            }
            let pts = self.engine.OnVideoStreamTick().ok()?;
            if pts == self.last {
                return None;
            }
            let whole = RECT {
                left: 0,
                top: 0,
                right: size[0] as i32,
                bottom: size[1] as i32,
            };
            self.engine
                .TransferVideoFrame(&surface.target, None, &whole, None)
                .ok()?;
            self.last = pts;

            // Down to memory the processor can read. `CopyResource` and then `Map` waits for the
            // copy to finish, which is a stall — sub-millisecond at panel size, and the reason the
            // size above is a panel's and not a film's. A ring of staging textures is what to
            // reach for if that ever stops being true.
            self.shared
                .context
                .CopyResource(&surface.staging, &surface.target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.shared
                .context
                .Map(
                    &surface.staging,
                    0,
                    D3D11_MAP_READ,
                    0,
                    Some(&mut mapped as *mut _),
                )
                .ok()?;
            let image = read_back(&mapped, size);
            self.shared.context.Unmap(&surface.staging, 0);
            Some(image)
        }
    }

    /// Make sure there is a pair of textures of exactly `size`. `Some(true)` if they are new.
    fn ensure(&mut self, size: [u32; 2]) -> Option<bool> {
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        if self.surface.as_ref().is_some_and(|held| held.size == size) {
            return Some(false);
        }
        let common = D3D11_TEXTURE2D_DESC {
            Width: size[0],
            Height: size[1],
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let target_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            ..common
        };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..common
        };
        let mut target = None;
        let mut staging = None;
        // SAFETY: both descriptions live for the calls, and both out-parameters are checked.
        unsafe {
            self.shared
                .device
                .CreateTexture2D(&target_desc, None, Some(&mut target))
                .ok()?;
            self.shared
                .device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .ok()?;
        }
        self.surface = Some(Surface {
            target: target?,
            staging: staging?,
            size,
        });
        Some(true)
    }
}

impl Drop for Engine {
    /// **`Shutdown` and not merely a release.**
    ///
    /// The engine holds a decoder, a presentation clock and the audio device, and dropping the
    /// last reference does not reliably give any of them back — a player left to be collected goes
    /// on being audible, which is the one failure in this file a user would report as a bug in the
    /// file manager rather than in a video. Every path that stops showing a video comes through
    /// here: closing the panel, moving the selection, shutting the tab, shutting the window.
    fn drop(&mut self) {
        // SAFETY: called once, on the thread that created the engine, and nothing touches it
        // afterwards.
        let _ = unsafe { self.engine.Shutdown() };
    }
}

/// The mapped staging texture as an image egui can upload.
///
/// Row by row, because `RowPitch` is the device's and is very often wider than the picture — a
/// straight copy of `width * height * 4` bytes would shear.
///
/// # SAFETY
///
/// `mapped` must describe a live mapping of at least `size` pixels at `RowPitch` bytes a row.
unsafe fn read_back(mapped: &D3D11_MAPPED_SUBRESOURCE, size: [u32; 2]) -> ColorImage {
    let (w, h) = (size[0] as usize, size[1] as usize);
    let pitch = mapped.RowPitch as usize;
    let base = mapped.pData as *const u8;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        // SAFETY: `y < h` and `x < w`, and the mapping covers `h` rows of `pitch` bytes.
        let row = unsafe { base.add(y * pitch) };
        for x in 0..w {
            let at = unsafe { row.add(x * 4) };
            // BGRA as the engine was asked for, RGBA as egui stores it, and opaque either way: a
            // video has no alpha, and forcing it here means a border pixel the engine did not
            // reach cannot come out as a transparent seam.
            pixels.push(Color32::from_rgb(
                unsafe { *at.add(2) },
                unsafe { *at.add(1) },
                unsafe { *at },
            ));
        }
    }
    ColorImage {
        size: [w, h],
        pixels,
        source_size: egui::vec2(w as f32, h as f32),
    }
}

/// A path as the URL `SetSource` wants.
///
/// Through `UrlCreateFromPathW` rather than by pasting `file:///` on the front, and that is the
/// whole reason this function exists: a URL has to escape `#`, `%`, `?` and a space, and a
/// filename may contain all four. Windows already knows the rules — and knows what to do with a
/// UNC path, which becomes `file://server/share/…` rather than anything a string concatenation
/// would produce.
fn url_for(path: &std::path::Path) -> Result<BSTR, String> {
    let wide = crate::shell::wide(path);
    // Room for the escaping, which can be three characters where the path had one, plus the
    // scheme. `INTERNET_MAX_URL_LENGTH` is the constant Windows uses for this and it is 2083;
    // this is a file path, so the generous multiple is cheaper than getting it wrong.
    let mut buffer = vec![0u16; wide.len() * 3 + 16];
    let mut len = buffer.len() as u32;
    // SAFETY: `wide` is null-terminated, and `len` says how much room `buffer` has. The call
    // writes at most that many units and updates `len` to how many it wrote.
    unsafe {
        UrlCreateFromPathW(
            PCWSTR(wide.as_ptr()),
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
            0,
        )
        .map_err(|_| "This video cannot be opened".to_owned())?;
    }
    buffer.truncate((len as usize).min(buffer.len()));
    Ok(BSTR::from_wide(&buffer))
}

/// The one complaint for every way the engine can refuse to be built.
///
/// Six calls in [`Engine::open`] can fail and not one of them tells a user anything: an attribute
/// store that could not be made and a class factory that is not registered are the same news —
/// this machine will not play videos — and the panel has one line to say it in.
fn trouble() -> String {
    "Video playback is not available".to_owned()
}
