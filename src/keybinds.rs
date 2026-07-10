#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Shortcut {
    pub(crate) key: egui::Key,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
}

impl Shortcut {
    pub(crate) const fn plain(key: egui::Key) -> Self {
        Self { key, ctrl: false, shift: false }
    }
    pub(crate) const fn shifted(key: egui::Key) -> Self {
        Self { key, ctrl: false, shift: true }
    }
    pub(crate) const fn ctrl(key: egui::Key) -> Self {
        Self { key, ctrl: true, shift: false }
    }
    pub(crate) fn label(self) -> String {
        let mut label = String::new();
        if self.ctrl {
            label.push_str("Ctrl+");
        }
        if self.shift {
            label.push_str("Shift+");
        }
        label.push_str(self.key.name());
        label
    }
}

pub(crate) fn shortcut_pressed(i: &egui::InputState, sc: Shortcut) -> bool {
    i.key_pressed(sc.key) && i.modifiers.ctrl == sc.ctrl && i.modifiers.shift == sc.shift
}

pub(crate) fn shortcut_down(i: &egui::InputState, sc: Shortcut) -> bool {
    i.key_down(sc.key) && i.modifiers.ctrl == sc.ctrl && i.modifiers.shift == sc.shift
}


/// A user-rebindable keyboard action.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Bind {
    SetStart,
    SetEnd,
    PlayPauseClip,
    PlayPause,
    SkipBack,
    SkipForward,
    StepBack,
    StepForward,
    Undo,
    Redo,
}

impl Bind {
    /// All actions, in display order (row-major: each consecutive pair forms one
    /// two-column grid row), with their settings labels.
    pub(crate) const ALL: [(Bind, &'static str); 10] = [
        (Bind::SetStart, "Set start"),
        (Bind::SetEnd, "Set end"),
        (Bind::PlayPauseClip, "Play / pause clip"),
        (Bind::PlayPause, "Play / pause"),
        (Bind::SkipForward, "Skip forward 5s"),
        (Bind::SkipBack, "Skip back 5s"),
        (Bind::StepForward, "Step forward 1 frame"),
        (Bind::StepBack, "Step back 1 frame"),
        (Bind::Undo, "Undo"),
        (Bind::Redo, "Redo"),
    ];

    /// Stable identifier for persistence, decoupled from display order so
    /// reordering or adding actions never misreads an older save.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Bind::SetStart => "set_start",
            Bind::SetEnd => "set_end",
            Bind::PlayPauseClip => "play_pause_clip",
            Bind::PlayPause => "play_pause",
            Bind::SkipBack => "skip_back",
            Bind::SkipForward => "skip_forward",
            Bind::StepBack => "step_back",
            Bind::StepForward => "step_forward",
            Bind::Undo => "undo",
            Bind::Redo => "redo",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Bind> {
        Bind::ALL.iter().map(|(b, _)| *b).find(|b| b.id() == id)
    }
}

/// The configurable shortcuts for the clip and playback actions.
#[derive(Clone, Copy)]
pub(crate) struct Keybinds {
    pub(crate) set_start: Shortcut,
    pub(crate) set_end: Shortcut,
    pub(crate) play_pause_clip: Shortcut,
    pub(crate) play_pause: Shortcut,
    pub(crate) skip_back: Shortcut,
    pub(crate) skip_forward: Shortcut,
    pub(crate) step_back: Shortcut,
    pub(crate) step_forward: Shortcut,
    pub(crate) undo: Shortcut,
    pub(crate) redo: Shortcut,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            set_start: Shortcut::plain(egui::Key::S),
            set_end: Shortcut::plain(egui::Key::E),
            play_pause_clip: Shortcut::plain(egui::Key::Space),
            play_pause: Shortcut::shifted(egui::Key::Space),
            skip_back: Shortcut::plain(egui::Key::ArrowLeft),
            skip_forward: Shortcut::plain(egui::Key::ArrowRight),
            step_back: Shortcut::shifted(egui::Key::ArrowLeft),
            step_forward: Shortcut::shifted(egui::Key::ArrowRight),
            undo: Shortcut::ctrl(egui::Key::Z),
            redo: Shortcut::ctrl(egui::Key::Y),
        }
    }
}

impl Keybinds {
    pub(crate) fn shortcut(&self, bind: Bind) -> Shortcut {
        match bind {
            Bind::SetStart => self.set_start,
            Bind::SetEnd => self.set_end,
            Bind::PlayPauseClip => self.play_pause_clip,
            Bind::PlayPause => self.play_pause,
            Bind::SkipBack => self.skip_back,
            Bind::SkipForward => self.skip_forward,
            Bind::StepBack => self.step_back,
            Bind::StepForward => self.step_forward,
            Bind::Undo => self.undo,
            Bind::Redo => self.redo,
        }
    }

    pub(crate) fn put(&mut self, bind: Bind, sc: Shortcut) {
        match bind {
            Bind::SetStart => self.set_start = sc,
            Bind::SetEnd => self.set_end = sc,
            Bind::PlayPauseClip => self.play_pause_clip = sc,
            Bind::PlayPause => self.play_pause = sc,
            Bind::SkipBack => self.skip_back = sc,
            Bind::SkipForward => self.skip_forward = sc,
            Bind::StepBack => self.step_back = sc,
            Bind::StepForward => self.step_forward = sc,
            Bind::Undo => self.undo = sc,
            Bind::Redo => self.redo = sc,
        }
    }

    /// Bind `sc` to `bind`. If another action already uses `sc`, swap so it takes
    /// `bind`'s old shortcut — keeping every action on a distinct shortcut.
    pub(crate) fn rebind(&mut self, bind: Bind, sc: Shortcut) {
        let old = self.shortcut(bind);
        for (other, _) in Bind::ALL {
            if other != bind && self.shortcut(other) == sc {
                self.put(other, old);
            }
        }
        self.put(bind, sc);
    }
}
