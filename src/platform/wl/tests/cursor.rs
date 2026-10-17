use super::super::wl_tablet::zwp_tablet_tool_v2::{self, Type};
use super::fixture::Fixture;
use wayland_backend::protocol::Argument;

#[test]
fn default_and_disabled_policies_send_no_cursor_request() {
    for builder in [
        crate::Builder::new(),
        crate::Builder::new().hide_tool_cursor(false),
    ] {
        let mut fixture = Fixture::new(builder);
        let pen = fixture.add_tool(Some(Type::Pen));
        for surface in 0..2 {
            fixture.enter(&pen, 10, surface);
            fixture.frame(&pen);
            assert_eq!(fixture.deliver(), ["in", "pose", "frame"]);
            assert!(fixture.state.cursors.is_empty());
            fixture.send(&pen, zwp_tablet_tool_v2::EVT_PROXIMITY_OUT_OPCODE, &[]);
            fixture.frame(&pen);
            fixture.deliver();
        }
    }
}

#[test]
fn hiding_covers_hover_all_tool_types_reentry_and_sibling_surfaces() {
    let mut fixture = Fixture::new(crate::Builder::new().hide_tool_cursor(true));
    for kind in [
        Some(Type::Pen),
        Some(Type::Eraser),
        Some(Type::Brush),
        Some(Type::Pencil),
        Some(Type::Airbrush),
        Some(Type::Finger),
        Some(Type::Lens),
        Some(Type::Mouse),
        None,
    ] {
        let tool = fixture.add_tool(kind);
        for surface in 0..2 {
            let serial = if surface == 0 {
                0x1234_5678
            } else {
                0x9876_5432
            };
            fixture.enter(&tool, serial, surface);
            fixture.frame(&tool);
            assert_eq!(fixture.deliver(), ["in", "pose", "frame"]);
            fixture.take_cursor(&tool, serial);
            fixture.send(&tool, zwp_tablet_tool_v2::EVT_PROXIMITY_OUT_OPCODE, &[]);
            fixture.frame(&tool);
            fixture.deliver();
            assert!(fixture.state.cursors.is_empty());
        }
    }
}

#[test]
fn hiding_preserves_contact_release_and_removal_events() {
    let mut fixture = Fixture::new(crate::Builder::new().hide_tool_cursor(true));
    let pen = fixture.add_tool(Some(Type::Pen));
    fixture.enter(&pen, 42, 0);
    fixture.send(
        &pen,
        zwp_tablet_tool_v2::EVT_DOWN_OPCODE,
        &[Argument::Uint(43)],
    );
    fixture.frame(&pen);
    assert_eq!(fixture.deliver(), ["in", "down", "pose", "frame"]);
    fixture.take_cursor(&pen, 42);
    fixture.send(&pen, zwp_tablet_tool_v2::EVT_UP_OPCODE, &[]);
    fixture.send(&pen, zwp_tablet_tool_v2::EVT_PROXIMITY_OUT_OPCODE, &[]);
    fixture.frame(&pen);
    fixture.send(&pen, zwp_tablet_tool_v2::EVT_REMOVED_OPCODE, &[]);
    assert_eq!(fixture.deliver(), ["pose", "up", "out", "frame", "removed"]);
    assert_eq!(fixture.manager.tools().len(), 1);
    fixture.manager.pump().unwrap();
    assert!(fixture.manager.tools().is_empty());
    assert!(fixture.state.cursors.is_empty());
}
