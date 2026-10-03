//! A video, playing: the one preview that is a live thing rather than a finished answer.
//!
//! Every other kind in [`super`] is read once on a worker and handed over as a [`super::Payload`]
//! — a decoded picture, a text body, a walked graph — and the panel then owns something that will
//! never change again. A video is the opposite: it has a clock, it makes sound, it holds a decoder
//! and an audio device, and it has to be **shut down** rather than merely dropped. So it does not
//! go through [`super::Previews`] at all. [`crate::app::App::collect_previews`] opens a [`Player`]
//! straight into the panel when the selection settles, and the panel owns it until the selection
//! moves.
//!
//! # The split
//!
//! This file is the policy: how big a frame to ask for, what the clock reads, what a click on the
//! play button means. [`win`] is the Media Foundation, and its header explains why the Media
//! Engine and why frame-server mode.
//!
//! # Why not on a worker
//!
//! Because there is nothing to put there. The engine is asynchronous by construction: opening a
//! file returns at once and the resolver reports back on Media Foundation's own threads, so the one
//! call that could block the window — opening the file — is already off it. What is left runs in
//! microseconds and has to be on the UI thread anyway, because a texture is the graphics device's.

use std::path::Path;

use egui::{TextureHandle, Vec2};

#[cfg(windows)]
#[path = "../windows/video.rs"]
mod win;

/// Off Windows there is no engine to drive, and [`super::kind_of`] never answers
/// [`super::Kind::Video`] — so no `Player` is ever built. This is the shape that lets the rest of
/// the module compile there rather than being `#[cfg]`-ed away from its call sites, which is the
/// trade [`super::visual`] makes with its `rendered` stub.
///
/// An empty enum rather than an empty struct, so every method below is `match *self {}` — a proof
/// that it cannot be reached rather than a return value somebody might come to rely on.
#[cfg(not(windows))]
mod win {
    pub(crate) enum Engine {}

    impl Engine {
        pub(crate) fn open(_path: &std::path::Path, _ctx: &egui::Context) -> Result<Self, String> {
            Err("Video playback needs Windows".to_owned())
        }
        pub(crate) fn words(&self) -> impl Iterator<Item = super::Word> + '_ {
            match *self {}
        }
        pub(crate) fn native(&self) -> Option<[u32; 2]> {
            match *self {}
        }
        pub(crate) fn duration(&self) -> f64 {
            match *self {}
        }
        pub(crate) fn at(&self) -> f64 {
            match *self {}
        }
        pub(crate) fn paused(&self) -> bool {
            match *self {}
        }
        pub(crate) fn play(&self) {
            match *self {}
        }
        pub(crate) fn pause(&self) {
            match *self {}
        }
        pub(crate) fn seek(&self, _to: f64) {
            match *self {}
        }
        pub(crate) fn set_muted(&self, _muted: bool) {
            match *self {}
        }
        pub(crate) fn has_audio(&self) -> bool {
            match *self {}
        }
        pub(crate) fn frame(&mut self, _size: [u32; 2]) -> Option<egui::ColorImage> {
            match *self {}
        }
    }
}

/// What the engine has said since the last frame.
///
/// Only the five things that change what is on screen. The clock is not among them: the panel reads
/// it off the engine on every frame it draws, so an event announcing that it moved would be an
/// event announcing what is already known.
pub enum Word {
    /// The metadata has landed: the picture's size and the running time are knowable.
    Ready,
    Playing,
    Paused,
    Ended,
    /// It will not play, and why — a sentence for the middle of the panel.
    Failed(String),
}

/// How the wanted frame size is quantised, as a fraction of the file's own.
///
/// Eight steps, and the reason for any quantisation at all is the splitter: the frame is asked for
/// at the size it will be drawn, the panel's size follows a drag continuously, and reallocating a
/// pair of textures on every frame of a drag would be the most expensive gesture in the window.
/// Eight steps means at most eight reallocations over the whole travel.
///
/// The scale is *ceiled* onto the step above, so the texture is never smaller than the canvas
/// wants — a frame scaled up by egui is soft, and this is a picture somebody is looking at.
const STEPS: f32 = 8.0;

/// How many frames a video gets to show something before the panel gives up on it.
///
/// Thirty seconds at sixty frames a second, and it is a frame count rather than a deadline on
/// purpose: the only frames counted are the ones [`Player::tick`] asked for, so a window that was
/// idle or minimised has not spent any of it. Generous, because the thing being waited for may be a
/// large file on a slow share and Media Foundation is entitled to take its time — what this is for is
/// the case where nothing is coming at all and nobody has said so.
const PATIENCE: u32 = 30 * 60;

/// How far an arrow key moves, in seconds.
pub const STEP: f64 = 10.0;

/// How often a held arrow key takes another [`STEP`].
///
/// **Its own cadence rather than the keyboard's repeat rate**, which is a setting on the machine and
/// has no business deciding how fast a video scrubs: at a typical thirty repeats a second, ten seconds
/// a repeat would cross a five-minute film in under a second. A step every eighth of a second is about
/// eighty seconds of travel a second — fast enough to get somewhere, slow enough to see where.
///
/// It also makes a *tap* exactly one step, whatever the machine's repeat delay is set to.
const STEP_EVERY: f64 = 0.125;

/// How many frames are asked for after a seek.
///
/// A seek while **paused** produces exactly one new frame, and nothing else would come and get it: the
/// window is idle in front of a paused video, so the frame that arrives a few milliseconds later would
/// wait for the next unrelated repaint. Which is a scrubber that moves and a picture that does not.
///
/// Eight, because the frame is not ready when the seek returns — the engine decodes it on its own
/// threads — and a handful of frames is both long enough to catch it and short enough to be idle again
/// before anybody notices the window was busy.
const NUDGE: u32 = 8;

/// One file, open and playing.
///
/// Holds the engine, the texture the frames land in, and the few facts that are worth remembering
/// rather than asking for. Everything else — where the clock is, whether it is paused — is read
/// from the engine on the frame that draws it, because the engine is the only thing that knows and
/// a copy would be a frame stale.
pub struct Player {
    engine: win::Engine,
    /// The frame on show. `None` until the first one has been transferred, which is a handful of
    /// frames after opening.
    texture: Option<TextureHandle>,
    /// The texture's own pixel size, which is what it is drawn at.
    shown: [u32; 2],
    /// The file's picture size, once the metadata has landed. `[0, 0]` before that, and for a file
    /// with a sound track and no picture.
    native: [u32; 2],
    /// How long it runs. `0.0` until known, and for something with no end.
    duration: f64,
    ready: bool,
    ended: bool,
    /// It will not play. The panel shows this instead of a canvas.
    failed: Option<String>,
    /// Where the scrubber is being dragged to, while it is being dragged.
    ///
    /// The engine is *not* asked to seek on every pixel of the drag — that would be a decode per
    /// pixel — so while a drag is in flight this is what the strip reads and the picture stays on
    /// the frame it was showing. The seek happens when the drag ends.
    scrubbing: Option<f64>,
    muted: bool,
    /// How many times the mute has actually been written to the engine.
    ///
    /// **For the test that it is written once and then left alone**, which is not the same claim as
    /// the value being right: the mute being correct at the end of a frame says nothing about how
    /// often it changed *during* one, and a mute rewritten every frame is audible as crackling rather
    /// than visible as a wrong value. Two owners writing it — a per-tile draw and a loop after the
    /// tiles — is exactly the bug this counts, and it made four clips in one panel crackle while
    /// every value assertion about them passed. See `crate::ui::preview::video::canvas`, which is the
    /// one owner there is now.
    #[cfg(test)]
    mute_writes: u32,
    /// How many frames have been asked for without anything to show for them. See [`PATIENCE`].
    waited: u32,
    /// **This player has the keyboard**, so Space and the arrows are its.
    ///
    /// Per player rather than one flag on the application, and it is what makes two panes each showing
    /// a video behave: a press anywhere tells *every* drawn canvas whether the pointer was over it, so
    /// clicking one video takes the keys off the other without either of them having to know the other
    /// exists. See `crate::ui::preview::video`, where the one line that does it lives.
    ///
    /// Nothing draws differently for it. A focus ring around a video would be a border around the
    /// picture, which is worse than not knowing — and the panel is beside the row it is about, so what
    /// has the keys is not in doubt once you have clicked it.
    keys: bool,
    /// When the last arrow step was taken, so a held key steps at [`STEP_EVERY`] rather than at
    /// whatever rate the machine repeats keys at.
    stepped_at: f64,
    /// A click waiting to find out whether it was half of a double click. See [`Self::clicked`].
    pending_click: Option<f64>,
    /// Frames still owed to a seek. See [`NUDGE`].
    nudge: u32,
}

impl Player {
    /// Open a file, ready to play.
    ///
    /// **Paused on its first frame, not playing.** The panel follows the keyboard, so a player that
    /// started by itself would mean a folder of clips performing one after another as you looked down
    /// it — and the loud half of that is not something to make somebody undo. What is loaded is
    /// enough to show: see [`win::Engine::open`], where the preload that produces a first frame
    /// without playing is the interesting part.
    pub fn open(path: &Path, muted: bool, ctx: &egui::Context) -> Result<Self, String> {
        let engine = win::Engine::open(path, ctx)?;
        engine.set_muted(muted);
        Ok(Self {
            engine,
            texture: None,
            shown: [0, 0],
            native: [0, 0],
            duration: 0.0,
            ready: false,
            ended: false,
            failed: None,
            scrubbing: None,
            muted,
            // The value it was born with does not count as a write: the engine was told at `open`,
            // and what this counts is the panel changing its mind afterwards.
            #[cfg(test)]
            mute_writes: 0,
            waited: 0,
            keys: false,
            stepped_at: f64::NEG_INFINITY,
            pending_click: None,
            nudge: 0,
        })
    }

    /// Collect what the engine has said, and put the frame it is showing into a texture.
    ///
    /// `canvas` is where the picture will be drawn, in points, and `scale` the display's — the two
    /// of them are what [`Self::wanted`] turns into a request. Called once per frame from the
    /// canvas that draws it, so the frame transferred is the frame drawn.
    ///
    /// Answers whether another frame should be asked for. **Nothing else asks**: this program is
    /// idle between events, so a playing video that did not say so would draw one frame and stop.
    pub fn tick(&mut self, ctx: &egui::Context, canvas: Vec2, scale: f32) -> bool {
        for word in self.engine.words().collect::<Vec<_>>() {
            match word {
                Word::Ready => {
                    self.ready = true;
                    self.native = self.engine.native().unwrap_or([0, 0]);
                    let duration = self.engine.duration();
                    // A stream has no end and an unopened file has no duration: both come back as
                    // something that is not a number, and both mean the same thing to the strip.
                    self.duration = if duration.is_finite() && duration > 0.0 {
                        duration
                    } else {
                        0.0
                    };
                }
                Word::Playing => self.ended = false,
                Word::Paused => {}
                Word::Ended => self.ended = true,
                // The *first* complaint is kept, not the last: it is the useful one, and the engine
                // often follows it with others as the graph tears itself down.
                Word::Failed(why) => {
                    self.failed.get_or_insert(why);
                }
            }
        }

        if self.failed.is_some() {
            return false;
        }

        // Only once the picture's size is known, which is what says there *is* a picture. **The
        // repaint below is decided separately**, and has to be: a `.mp4` holding nothing but an AAC
        // track never gets a frame, and an early return here would leave the strip's clock frozen
        // over a file that was audibly playing.
        if self.native != [0, 0] {
            let wanted = wanted(self.native, canvas, scale);
            if let Some(image) = self.engine.frame(wanted) {
                self.shown = wanted;
                match &mut self.texture {
                    // **Set, not loaded.** `Context::load_texture` allocates a new texture and frees
                    // the old one every time it is called, which at thirty frames a second is a
                    // texture churned per frame — the shape of a leak even when it is not one.
                    Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        self.texture = Some(ctx.load_texture(
                            "preview-video",
                            image,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                }
            }
        }

        // **And give up on a file that is going nowhere.** Everything above is driven by frames this
        // function asks for, so a video that never produces one spins the window at the display's
        // rate for as long as the panel is open — which is a flat battery rather than a bug anybody
        // would report. The engine says so itself for every failure it *notices*; this is the floor
        // under the ones it does not, and it can only trip while the frames it counts are being
        // asked for, which makes it a wall clock without being one.
        if self.opening() {
            self.waited += 1;
            if self.waited > PATIENCE {
                self.failed = Some("This video cannot be played".to_owned());
                return false;
            }
        }

        // While it is playing, and not otherwise. A paused video is a still picture and the window
        // should go back to being idle in front of it — but one that has been *asked* to play and has
        // not shown anything yet has to keep the frames coming or it never will.
        //
        // And for a few frames after a seek, whatever the state: the frame a seek produces arrives
        // milliseconds later, and nothing else would come back for it. See [`NUDGE`].
        self.nudge = self.nudge.saturating_sub(1);
        !self.engine.paused() || self.opening() || self.nudge > 0
    }

    /// The frame to draw, and its pixel size. `None` before the first one has arrived.
    pub fn frame(&self) -> Option<(&TextureHandle, Vec2)> {
        let texture = self.texture.as_ref()?;
        Some((
            texture,
            Vec2::new(self.shown[0] as f32, self.shown[1] as f32),
        ))
    }

    /// Why it will not play, if it will not.
    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// Whether it is still on its way to showing anything.
    ///
    /// **Not simply "has no frame yet"**, and the difference is a file with a sound track and no
    /// picture — a voice memo in a `.mp4`, a ripped soundtrack in an `.mkv`. A frame is never coming
    /// for one of those, so the obvious test never clears: the canvas would sit on `Opening…` for as
    /// long as the file played, and `--shot --preview` would wait out its whole patience on it.
    ///
    /// Which is why this is one question asked in one place. The canvas draws from it and
    /// [`crate::ui::preview::Preview::busy`] waits on it, and those two have to agree or one of them
    /// is wrong about the same file.
    pub fn opening(&self) -> bool {
        self.failed.is_none() && self.texture.is_none() && !(self.ready && self.native == [0, 0])
    }

    /// The file's picture size, for the bar. `None` for a file with no picture in it.
    pub fn native(&self) -> Option<[u32; 2]> {
        (self.native != [0, 0]).then_some(self.native)
    }

    /// How long it runs, in seconds. `None` for a stream, or before the metadata lands.
    pub fn duration(&self) -> Option<f64> {
        (self.duration > 0.0).then_some(self.duration)
    }

    /// Where it has got to, in seconds — or where it is being dragged to, while it is.
    pub fn at(&self) -> f64 {
        self.scrubbing.unwrap_or_else(|| self.engine.at())
    }

    pub fn playing(&self) -> bool {
        !self.engine.paused()
    }

    /// Play, pause, or — at the end — start again.
    ///
    /// The third case is what makes the button worth pressing after a clip has finished: an engine
    /// sitting at its own last frame answers `Play` by doing nothing at all, so the seek has to come
    /// first.
    pub fn toggle(&mut self) {
        if self.ended {
            self.ended = false;
            self.engine.seek(0.0);
            self.engine.play();
        } else if self.engine.paused() {
            self.engine.play();
        } else {
            self.engine.pause();
        }
    }

    /// The scrubber is being dragged: show `to` without decoding it yet.
    pub fn scrub_to(&mut self, to: f64) {
        self.scrubbing = Some(to.clamp(0.0, self.duration));
    }

    /// And let go: go to wherever it ended up.
    ///
    /// A seek while paused still shows the frame it lands on — the engine decodes it and reports a
    /// new timestamp — so scrubbing a paused video works, which is most of what scrubbing is for.
    pub fn scrub_done(&mut self) {
        if let Some(to) = self.scrubbing.take() {
            self.seek(to);
        }
    }

    /// Go straight to `to` seconds: a click on the scrubber's track rather than a drag.
    pub fn seek(&mut self, to: f64) {
        self.ended = false;
        self.nudge = NUDGE;
        self.engine.seek(to.clamp(0.0, self.duration));
    }

    /// Whether this player has the keyboard. See [`Self::keys`].
    pub fn has_keys(&self) -> bool {
        self.keys
    }

    /// A press landed: on this video, or somewhere else.
    pub fn pressed_on(&mut self, mine: bool) {
        self.keys = mine;
        // A press elsewhere is also the end of any click this canvas was still holding — the click
        // that started it belongs to whatever was pressed next.
        if !mine {
            self.pending_click = None;
        }
    }

    /// Step `by` seconds, forward or back, if enough time has passed since the last step.
    ///
    /// **Rate-limited here rather than at the keyboard**, because the thing being limited is the
    /// *engine*: every step is a seek, a seek is a decode, and a held arrow key at the machine's repeat
    /// rate would ask for thirty of them a second and show none. See [`STEP_EVERY`].
    ///
    /// `now` is egui's clock, which is the same one the caller reads the keys from.
    pub fn step(&mut self, by: f64, now: f64) {
        if now - self.stepped_at < STEP_EVERY {
            return;
        }
        self.stepped_at = now;
        let to = (self.at() + by).clamp(0.0, self.duration.max(0.0));
        // A step is a position somebody chose, so it also lets go of a scrub in flight rather than
        // fighting it.
        self.scrubbing = None;
        self.seek(to);
    }

    /// A click on the picture, which **may turn out to be half of a double click**.
    ///
    /// So it is not acted on yet. egui reports the first press of a double click as an ordinary click
    /// and only names the second one, so playing on the first press means a double click plays *and*
    /// fills the screen — which is a video that pauses itself for having been maximised. Waiting one
    /// double-click delay is the only way to tell the two apart, and it is why play/pause from the
    /// picture is a fraction slower than from the button in the strip, which has no such ambiguity.
    pub fn clicked(&mut self, now: f64) {
        self.pending_click = Some(now);
    }

    /// It *was* a double click, so the click before it was never a play.
    pub fn double_clicked(&mut self) {
        self.pending_click = None;
    }

    /// Whether a click is still waiting to become a play, and how long is left of the wait.
    ///
    /// Both, because the caller has to book the frame that would notice — the same shape
    /// [`crate::ui::preview::Preview::settle`] has, for the same reason: this program is idle between
    /// events, so a wait nothing asks to be woken from is a wait that never ends.
    pub fn settle_click(&mut self, now: f64, delay: f64) -> Option<f64> {
        let at = self.pending_click?;
        let left = delay - (now - at);
        if left > 0.0 {
            return Some(left);
        }
        self.pending_click = None;
        self.toggle();
        None
    }

    /// Whether there is a sound track, so the mute button is only offered where it does something.
    ///
    /// Not knowable before the metadata lands, and `false` then — which is the right way round for a
    /// control that appears: a button that flickers into existence a moment after the panel fills is
    /// better than one that was there and did nothing.
    pub fn has_audio(&self) -> bool {
        self.ready && self.engine.has_audio()
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.engine.set_muted(muted);
        #[cfg(test)]
        {
            self.mute_writes += 1;
        }
    }

    /// How many times the mute has reached the engine. See [`Player::mute_writes`].
    #[cfg(test)]
    pub fn mute_writes(&self) -> u32 {
        self.mute_writes
    }

    /// Stop, because nobody is looking any more.
    ///
    /// **The tab has stopped being the active one**, which is the one way a player can end up out
    /// of sight while still being held: an inactive tab is not drawn and not followed, so nothing
    /// would tick it — and nothing would stop it either, and it would go on filling the room with
    /// the sound of a video nobody can see. See [`crate::app::App::collect_previews`].
    pub fn hidden(&mut self) {
        // Asked before it is told, because this runs for every inactive tab on every frame: a getter
        // is cheap and a state change on an engine that is already paused is not free.
        if !self.engine.paused() {
            self.engine.pause();
        }
    }
}

/// The frame size to ask the engine for, in pixels.
///
/// **The size it will be drawn at, not the file's own**, and that is the bound that makes this whole
/// approach affordable: every frame is copied back from the graphics device into memory, so a 4K film
/// would be 33 MB a frame at its own size and is a few hundred kilobytes at a panel's.
///
/// Three rules, in order.
///
/// - **Never larger than the file.** Enlarging on the graphics device and then again in egui is two
///   blurs for the price of one, so a 320 × 240 clip is fetched at 320 × 240 however big the panel
///   is, and the canvas is what stretches it.
/// - **Never larger than [`super::CAP`]**, the same ceiling a decoded picture gets. For a 4K file on
///   a 4K display this is the rule that actually decides the answer.
/// - **Quantised onto [`STEPS`].** The panel's size follows a splitter drag continuously, and a pair
///   of textures reallocated on every frame of a drag would make it the most expensive gesture in the
///   window. Eight steps means at most eight reallocations across the whole travel — and the scale is
///   *ceiled* onto the step above, so the texture is never smaller than the canvas asked for.
///
/// One scale factor for both edges throughout, so the aspect ratio stays the file's: the canvas draws
/// this texture into a rect it has fitted, and a texture whose edges had been rounded independently
/// would be a picture stretched by a pixel or two.
///
/// A free function rather than a method because it is the one piece of arithmetic here worth pinning
/// down, and a [`Player`] cannot be built without a graphics device and a file to play.
pub fn wanted(native: [u32; 2], canvas: Vec2, scale: f32) -> [u32; 2] {
    let native = Vec2::new(native[0] as f32, native[1] as f32);
    if native.x < 1.0 || native.y < 1.0 {
        return [0, 0];
    }
    // Floored at something, because a panel dragged to nothing still has to ask for a frame the
    // engine will accept — and a zero-sized texture is not one.
    let room = (canvas * scale.max(0.1)).max(Vec2::splat(16.0));
    let fits = (room.x / native.x).min(room.y / native.y).min(1.0);
    let stepped = ((fits * STEPS).ceil() / STEPS).clamp(1.0 / STEPS, 1.0);
    let ceiling = (super::CAP as f32 / native.x.max(native.y)).min(1.0);
    let at = stepped.min(ceiling);
    [
        ((native.x * at).round() as u32).max(1),
        ((native.y * at).round() as u32).max(1),
    ]
}

/// Seconds as a clock: `0:07`, `4:12`, `1:03:20`.
///
/// Hours only when there are hours, which is nearly never for something being previewed and is
/// jarring when it is padded for — `0:00:07` reads as a stopwatch rather than a position in a clip.
pub fn clock(seconds: f64) -> String {
    let whole = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    let (h, m, s) = (whole / 3600, (whole / 60) % 60, whole % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
