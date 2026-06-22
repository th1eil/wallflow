/// Loop behaviour for dynamic wallpaper playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    /// Loop forever (default)
    Infinite,
    /// Play from first to last frame once, then stop
    PlayOnce,
    /// Play forward then backward, repeating
    PingPong,
}

/// Commands sent from the main thread to a running dynamic/video player task.
#[derive(Debug, Clone)]
pub enum PlaybackCommand {
    Pause,
    Resume,
    SetLoopMode(LoopMode),
    /// Override native frame timing; e.g. 24 = 24 fps
    SetFps(u32),
    Stop,
}

#[allow(dead_code)]
/// Current state of a dynamic wallpaper player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}
