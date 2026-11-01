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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PasteRoute {
    Hyprland(PasteShortcut),
    VirtualKeyboard(PasteShortcut),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PasteRouteContext {
    pub global_paste_shortcut: bool,
    pub default_shift_paste: bool,
    pub class_hint: Option<PasteShortcut>,
    pub hyprland_available: bool,
    pub wayland_env: bool,
}

/// Select once, before publishing clipboard contents. A failed dispatch must
/// never advance to another route because the first may already have inserted.
pub(super) fn plan_paste_route(context: PasteRouteContext) -> Option<PasteRoute> {
    let shortcut = match context.class_hint {
        _ if context.global_paste_shortcut => PasteShortcut::ShiftInsert,
        Some(PasteShortcut::ShiftInsert) => PasteShortcut::ShiftInsert,
        Some(PasteShortcut::CtrlShiftV) => PasteShortcut::CtrlShiftV,
        _ if context.default_shift_paste => PasteShortcut::CtrlShiftV,
        _ => PasteShortcut::CtrlV,
    };

    if context.hyprland_available {
        Some(PasteRoute::Hyprland(shortcut))
    } else if context.wayland_env {
        Some(PasteRoute::VirtualKeyboard(shortcut))
    } else {
        None
    }
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
    fn unsupported_session_has_no_paste_routes() {
        assert!(plan_paste_route(PasteRouteContext {
            global_paste_shortcut: false,
            default_shift_paste: false,
            class_hint: None,
            hyprland_available: false,
            wayland_env: false,
        })
        .is_none());
    }

    #[test]
    fn every_candidate_preserves_the_selected_shortcut() {
        let route = plan_paste_route(PasteRouteContext {
            global_paste_shortcut: false,
            default_shift_paste: false,
            class_hint: None,
            hyprland_available: false,
            wayland_env: true,
        });
        assert_eq!(
            route,
            Some(PasteRoute::VirtualKeyboard(PasteShortcut::CtrlV))
        );
    }

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
            plan_paste_route(PasteRouteContext {
                global_paste_shortcut: true,
                default_shift_paste: false,
                class_hint: None,
                hyprland_available: true,
                wayland_env: true,
            }),
            Some(PasteRoute::Hyprland(PasteShortcut::ShiftInsert))
        );
    }

    #[test]
    fn default_paste_selects_hyprland_before_dispatch() {
        assert_eq!(
            plan_paste_route(PasteRouteContext {
                global_paste_shortcut: false,
                default_shift_paste: false,
                class_hint: None,
                hyprland_available: true,
                wayland_env: true,
            }),
            Some(PasteRoute::Hyprland(PasteShortcut::CtrlV))
        );
    }
}
