use std::collections::HashSet;

const SHIFT_PASTE_CLASSES: &[&str] = &[
    "Alacritty",
    "kitty",
    "foot",
    "footclient",
    "WezTerm",
    "org.wezfurlong.wezterm",
    "org.gnome.Console",
    "gnome-terminal-server",
    "konsole",
    "yakuake",
    "terminator",
    "tilix",
    "termite",
    "xfce4-terminal",
    "wezterm-gui",
    "rio",
    "WarpTerminal",
    "xterm",
    "urxvt",
    "Ghostty",
    "ghostty",
    "com.mitchellh.ghostty",
];

const SHIFT_PASTE_CLASS_COMPONENTS: &[&str] = &[
    "terminal",
    "console",
    "ghostty",
    "wezterm",
    "kitty",
    "alacritty",
    "warpterminal",
    "rio",
    "foot",
    "konsole",
    "xterm",
    "urxvt",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PasteShortcut {
    CtrlV,
    CtrlShiftV,
    ShiftInsert,
}

impl PasteShortcut {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::CtrlV => "ctrl+v",
            Self::CtrlShiftV => "ctrl+shift+v",
            Self::ShiftInsert => "shift+insert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PasteRoute {
    Hyprland(PasteShortcut),
    VirtualKeyboard(PasteShortcut),
    Enigo(PasteShortcut),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PasteRouteContext {
    pub global_paste_shortcut: bool,
    pub default_shift_paste: bool,
    pub class_hint: Option<PasteShortcut>,
    pub hyprland_available: bool,
    pub wayland_env: bool,
}

pub(super) fn plan_paste_routes(context: PasteRouteContext) -> Vec<PasteRoute> {
    if context.global_paste_shortcut || context.class_hint == Some(PasteShortcut::ShiftInsert) {
        return shift_insert_routes(context.hyprland_available, context.wayland_env);
    }

    let shortcut = match context.class_hint {
        Some(PasteShortcut::CtrlShiftV) => PasteShortcut::CtrlShiftV,
        _ if context.default_shift_paste => PasteShortcut::CtrlShiftV,
        _ => PasteShortcut::CtrlV,
    };

    let mut routes = Vec::new();
    if context.hyprland_available {
        routes.push(PasteRoute::Hyprland(shortcut));
    }
    if context.wayland_env {
        routes.push(PasteRoute::VirtualKeyboard(shortcut));
    }
    // Preserve the historical X/Wayland fallback: Ctrl+Shift+V is more likely
    // to paste into terminals than Ctrl+V after higher-confidence routes fail.
    routes.push(PasteRoute::Enigo(PasteShortcut::CtrlShiftV));
    routes
}

fn shift_insert_routes(hyprland_available: bool, wayland_env: bool) -> Vec<PasteRoute> {
    let mut routes = Vec::new();
    if hyprland_available {
        routes.push(PasteRoute::Hyprland(PasteShortcut::ShiftInsert));
    }
    if wayland_env {
        routes.push(PasteRoute::VirtualKeyboard(PasteShortcut::ShiftInsert));
    }
    routes.push(PasteRoute::Enigo(PasteShortcut::ShiftInsert));
    routes
}

pub(super) fn class_paste_hint_for_class(
    class: &str,
    extra_shift_classes: &[String],
    extra_shift_insert_classes: &[String],
) -> Option<PasteShortcut> {
    let shift_index = class_hint_index(class, extra_shift_classes);
    let shift_insert_index = class_hint_index(class, extra_shift_insert_classes);

    match (shift_index, shift_insert_index) {
        (Some(shift), Some(shift_insert)) => {
            if shift <= shift_insert {
                Some(PasteShortcut::CtrlShiftV)
            } else {
                Some(PasteShortcut::ShiftInsert)
            }
        }
        (Some(_), None) => Some(PasteShortcut::CtrlShiftV),
        (None, Some(_)) => Some(PasteShortcut::ShiftInsert),
        (None, None) => {
            if built_in_shift_hint_for_class(class) {
                Some(PasteShortcut::CtrlShiftV)
            } else {
                None
            }
        }
    }
}

fn built_in_shift_hint_for_class(class: &str) -> bool {
    if SHIFT_PASTE_CLASSES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(class))
    {
        return true;
    }

    let lower = class.to_ascii_lowercase();
    for component in lower.split(['.', '-', '_']) {
        if SHIFT_PASTE_CLASS_COMPONENTS.iter().any(|c| c == &component) {
            return true;
        }
    }

    false
}

fn class_hint_index(class: &str, hints: &[String]) -> Option<usize> {
    if hints.is_empty() {
        return None;
    }

    let lower = class.to_ascii_lowercase();
    let components: HashSet<&str> = lower.split(['.', '-', '_']).collect();
    hints.iter().position(|hint| {
        if hint == &lower {
            return true;
        }
        components.contains(hint.as_str())
    })
}

pub(super) fn normalize_hint_classes(entries: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for entry in entries {
        let normalized_entry = entry.trim().to_ascii_lowercase();
        if !normalized_entry.is_empty() && seen.insert(normalized_entry.clone()) {
            normalized.push(normalized_entry);
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_paste_hint_uses_first_class_instantiation() {
        let shift_classes = normalize_hint_classes(vec!["foo".to_string(), "zed".to_string()]);
        let shift_insert_classes =
            normalize_hint_classes(vec!["zed".to_string(), "bar".to_string()]);

        assert_eq!(
            class_paste_hint_for_class("dev.zed.Zed", &shift_classes, &shift_insert_classes),
            Some(PasteShortcut::ShiftInsert)
        );
    }

    #[test]
    fn class_hint_lookup_supports_component_names() {
        let hints = normalize_hint_classes(vec!["zed".to_string()]);
        assert_eq!(class_hint_index("dev.zed.Zed", &hints), Some(0));
    }

    #[test]
    fn class_paste_hint_falls_back_to_built_in_terminal_rules() {
        let shift_classes = normalize_hint_classes(Vec::new());
        let shift_insert_classes = normalize_hint_classes(Vec::new());
        assert_eq!(
            class_paste_hint_for_class("kitty", &shift_classes, &shift_insert_classes),
            Some(PasteShortcut::CtrlShiftV)
        );
    }

    #[test]
    fn normalize_hint_classes_dedupes_preserving_first_order() {
        let normalized = normalize_hint_classes(vec![
            "ZED".to_string(),
            "zed".to_string(),
            " dev.zed.zed ".to_string(),
            "".to_string(),
        ]);
        assert_eq!(
            normalized,
            vec!["zed".to_string(), "dev.zed.zed".to_string()]
        );
    }

    #[test]
    fn global_paste_plans_shift_insert_routes() {
        assert_eq!(
            plan_paste_routes(PasteRouteContext {
                global_paste_shortcut: true,
                default_shift_paste: false,
                class_hint: None,
                hyprland_available: true,
                wayland_env: true,
            }),
            vec![
                PasteRoute::Hyprland(PasteShortcut::ShiftInsert),
                PasteRoute::VirtualKeyboard(PasteShortcut::ShiftInsert),
                PasteRoute::Enigo(PasteShortcut::ShiftInsert),
            ]
        );
    }

    #[test]
    fn default_paste_plans_hyprland_then_virtual_then_enigo_fallback() {
        assert_eq!(
            plan_paste_routes(PasteRouteContext {
                global_paste_shortcut: false,
                default_shift_paste: false,
                class_hint: None,
                hyprland_available: true,
                wayland_env: true,
            }),
            vec![
                PasteRoute::Hyprland(PasteShortcut::CtrlV),
                PasteRoute::VirtualKeyboard(PasteShortcut::CtrlV),
                PasteRoute::Enigo(PasteShortcut::CtrlShiftV),
            ]
        );
    }
}
