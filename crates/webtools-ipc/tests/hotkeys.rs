use webtools_ipc::hotkey::{Binding, DoubleTap};

#[test]
fn existing_wpf_shortcut_forms_resolve_to_windows_keys() {
    assert_eq!(
        Binding::parse("Control+Alt+Space"),
        Some(Binding::Chord {
            modifiers: 3,
            key: 0x20
        })
    );
    assert_eq!(
        Binding::parse("ctrl+shift+f12"),
        Some(Binding::Chord {
            modifiers: 6,
            key: 0x7b
        })
    );
    assert_eq!(
        Binding::parse("FunctionKey:F10"),
        Some(Binding::Chord {
            modifiers: 0,
            key: 0x79
        })
    );
    assert_eq!(
        Binding::parse("DoubleModifier:Alt"),
        Some(Binding::DoubleModifier { key: 0x12 })
    );
    for invalid in [
        "F11",
        "Ctrl+Control+A",
        "Alt+Q+W",
        "DoubleModifier:Shift",
        "bad",
    ] {
        assert_eq!(Binding::parse(invalid), None, "{invalid}");
    }
}

#[test]
fn double_tap_requires_two_short_clean_releases_and_never_triggers_on_a_chord() {
    let mut gesture = DoubleTap::new(0x11);
    assert!(!gesture.key(0xa2, true, 0));
    assert!(!gesture.key(0xa2, false, 50));
    assert!(!gesture.key(0xa3, true, 130));
    assert!(gesture.key(0xa3, false, 180));
    assert!(!gesture.key(0x11, true, 210));
    assert!(!gesture.key(0x43, true, 220));
    assert!(!gesture.key(0x43, false, 230));
    assert!(!gesture.key(0x11, false, 240));
    assert!(!gesture.key(0x11, true, 260));
    assert!(!gesture.key(0x11, false, 280));
    assert!(!gesture.key(0x11, true, 300));
    assert!(!gesture.key(0x11, false, 650));
}

#[test]
fn double_tap_expires_and_repeated_keydown_invalidates_the_tap() {
    let mut gesture = DoubleTap::new(0x12);
    for (key, down, time) in [
        (0x12, true, 0),
        (0x12, false, 40),
        (0x12, true, 500),
        (0x12, true, 520),
        (0x12, false, 540),
        (0x12, true, 560),
        (0x12, false, 600),
    ] {
        assert!(!gesture.key(key, down, time));
    }
}
