use super::*;
use wayland_client::protocol::wl_surface::WlSurface;
use wl_tablet::zwp_tablet_tool_v2::{Event as ToolMessage, ZwpTabletToolV2};

mod cursor;
mod fixture;
mod wake;

struct Input {
    state: TabletState,
    conn: Connection,
    queue: wayland_client::EventQueue<TabletState>,
    _server: std::os::unix::net::UnixStream,
}

impl Input {
    fn new() -> Self {
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let conn = Connection::from_socket(client).unwrap();
        let queue = conn.new_event_queue();
        Self {
            state: TabletState::default(),
            conn,
            queue,
            _server: server,
        }
    }

    fn send(&mut self, event: ToolMessage) {
        let tool = ZwpTabletToolV2::inert(self.conn.backend().downgrade());
        <TabletState as Dispatch<ZwpTabletToolV2, ()>>::event(
            &mut self.state,
            &tool,
            event,
            &(),
            &self.conn,
            &self.queue.handle(),
        );
    }

    fn enter(&mut self) {
        self.send(ToolMessage::ProximityIn {
            serial: 1,
            tablet: wl_tablet::zwp_tablet_v2::ZwpTabletV2::inert(self.conn.backend().downgrade()),
            surface: WlSurface::inert(self.conn.backend().downgrade()),
        });
        self.send(ToolMessage::Motion { x: 10.0, y: 20.0 });
    }

    fn pose(&self) -> Pose {
        self.state
            .events
            .iter()
            .find_map(|event| match event {
                raw_events::Event::Tool {
                    event: raw_events::ToolEvent::Pose(pose),
                    ..
                } => Some(*pose),
                _ => None,
            })
            .expect("frame must contain a pose")
    }

    fn event_kinds(&self) -> Vec<&'static str> {
        self.state
            .events
            .iter()
            .map(|event| match event {
                raw_events::Event::Tool { event, .. } => match event {
                    raw_events::ToolEvent::In { .. } => "in",
                    raw_events::ToolEvent::Down => "down",
                    raw_events::ToolEvent::Pose(_) => "pose",
                    raw_events::ToolEvent::Up => "up",
                    raw_events::ToolEvent::Out => "out",
                    raw_events::ToolEvent::Frame(_) => "frame",
                    _ => "other tool event",
                },
                _ => "other event",
            })
            .collect()
    }
}

#[test]
fn proximity_and_contact_survive_the_same_frame() {
    let mut input = Input::new();
    input.enter();
    input.send(ToolMessage::Down { serial: 2 });
    input.send(ToolMessage::Frame { time: 1 });
    assert_eq!(input.event_kinds(), ["in", "down", "pose", "frame"]);
}

#[test]
fn release_and_proximity_out_survive_the_same_frame() {
    let mut input = Input::new();
    input.enter();
    input.send(ToolMessage::Down { serial: 2 });
    input.send(ToolMessage::Frame { time: 1 });
    input.state.cleanup_start();
    input.send(ToolMessage::Up);
    input.send(ToolMessage::ProximityOut);
    input.send(ToolMessage::Frame { time: 2 });
    assert_eq!(input.event_kinds(), ["pose", "up", "out", "frame"]);
}

#[test]
fn transitions_and_pose_are_cleared_after_proximity_out() {
    let mut input = Input::new();
    input.enter();
    input.send(ToolMessage::Up);
    input.send(ToolMessage::ProximityOut);
    input.send(ToolMessage::Frame { time: 1 });
    input.state.cleanup_start();
    input.send(ToolMessage::Frame { time: 2 });
    assert_eq!(input.event_kinds(), ["frame"]);
}

#[test]
fn slider_preserves_signed_endpoints_and_neutral_position() {
    let mut input = Input::new();
    input.enter();
    for (position, expected) in [(-65535, -1.0), (0, 0.0), (65535, 1.0)] {
        input.state.cleanup_start();
        input.send(ToolMessage::Slider { position });
        input.send(ToolMessage::Frame { time: 1 });
        assert_eq!(
            input.pose().slider.get(),
            Some(expected),
            "raw slider {position}"
        );
    }
}

#[test]
fn wheel_accumulates_deltas_across_messages_and_frames() {
    let mut input = Input::new();
    input.enter();
    input.send(ToolMessage::Wheel {
        degrees: 15.0,
        clicks: 1,
    });
    input.send(ToolMessage::Wheel {
        degrees: 15.0,
        clicks: 2,
    });
    input.send(ToolMessage::Frame { time: 1 });
    assert_eq!(input.pose().wheel, Some((30.0_f32.to_radians(), 3)));
    input.state.cleanup_start();
    input.send(ToolMessage::Wheel {
        degrees: -15.0,
        clicks: -1,
    });
    input.send(ToolMessage::Frame { time: 2 });
    assert_eq!(input.pose().wheel, Some((15.0_f32.to_radians(), 2)));
}

#[test]
fn wheel_does_not_reapply_a_delta_on_a_later_frame() {
    let mut input = Input::new();
    input.enter();
    input.send(ToolMessage::Wheel {
        degrees: 15.0,
        clicks: 1,
    });
    input.send(ToolMessage::Frame { time: 1 });
    input.state.cleanup_start();
    input.send(ToolMessage::Frame { time: 2 });
    assert_eq!(input.pose().wheel, Some((15.0_f32.to_radians(), 1)));
}

#[test]
fn strip_events_follow_the_group_to_the_parent_pad() {
    use wl_tablet::zwp_tablet_pad_strip_v2::{Event as StripMessage, ZwpTabletPadStripV2};
    let mut input = Input::new();
    let qh = input.queue.handle();
    let strip = ZwpTabletPadStripV2::inert(input.conn.backend().downgrade());
    let group = input.conn.display().id();
    let pad = input.conn.display().get_registry(&qh, ()).id();
    input
        .state
        .strip_associations
        .insert(strip.id(), group.clone());
    input
        .state
        .group_associations
        .insert(group.clone(), pad.clone());
    for message in [
        StripMessage::Position { position: 65535 },
        StripMessage::Source {
            source: wayland_client::WEnum::Value(
                wl_tablet::zwp_tablet_pad_strip_v2::Source::Finger,
            ),
        },
        StripMessage::Stop,
        StripMessage::Frame { time: 1 },
    ] {
        <TabletState as Dispatch<ZwpTabletPadStripV2, ()>>::event(
            &mut input.state,
            &strip,
            message,
            &(),
            &input.conn,
            &qh,
        );
    }
    let events: Vec<_> = input
        .state
        .events
        .iter()
        .map(|event| match event {
            raw_events::Event::Pad {
                pad: parent,
                event:
                    raw_events::PadEvent::Group {
                        group: parent_group,
                        event: raw_events::PadGroupEvent::Strip { event, .. },
                    },
            } if parent == &pad && parent_group == &group => match event {
                crate::events::TouchStripEvent::Pose(value)
                    if value.to_bits() == 1.0_f32.to_bits() =>
                {
                    "pose"
                }
                crate::events::TouchStripEvent::Source(TouchSource::Finger) => "source",
                crate::events::TouchStripEvent::Up => "up",
                crate::events::TouchStripEvent::Frame(_) => "frame",
                _ => "unexpected strip value",
            },
            _ => "wrong route",
        })
        .collect();
    assert_eq!(events, ["pose", "source", "up", "frame"]);
}
