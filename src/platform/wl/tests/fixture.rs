//! Protocol tests using the real libwayland client and a borrowed display.
use super::super::*;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WaylandDisplayHandle,
    WaylandWindowHandle, WindowHandle,
};
use std::{os::fd::OwnedFd, os::unix::net::UnixStream, ptr::NonNull, sync::Arc};
use wayland_backend::{
    message,
    protocol::{Argument, Message},
    server,
};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface};
use wl_tablet::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_seat_v2,
    zwp_tablet_tool_v2::{self, Type, ZwpTabletToolV2},
    zwp_tablet_v2::{self, ZwpTabletV2},
};

#[derive(Debug)]
pub(super) struct CursorRequest {
    tool: server::ObjectId,
    serial: u32,
    surface: server::ObjectId,
    hotspot: [i32; 2],
}

#[derive(Default)]
pub(super) struct Compositor {
    surfaces: Vec<server::ObjectId>,
    tablet_seat: Option<server::ObjectId>,
    pub(super) cursors: Vec<CursorRequest>,
}

struct Object;

impl server::GlobalHandler<Compositor> for Object {
    fn bind(
        self: Arc<Self>,
        _: &server::Handle,
        _: &mut Compositor,
        _: server::ClientId,
        _: server::GlobalId,
        _: server::ObjectId,
    ) -> Arc<dyn server::ObjectData<Compositor>> {
        self
    }
}

impl server::ObjectData<Compositor> for Object {
    fn request(
        self: Arc<Self>,
        _: &server::Handle,
        state: &mut Compositor,
        _: server::ClientId,
        msg: Message<server::ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn server::ObjectData<Compositor>>> {
        match (msg.sender_id.interface().name, msg.opcode, &msg.args[..]) {
            ("wl_compositor", 0, [Argument::NewId(surface)]) => {
                state.surfaces.push(surface.clone());
                Some(self)
            }
            ("zwp_tablet_manager_v2", 0, [Argument::NewId(seat), Argument::Object(_)]) => {
                state.tablet_seat = Some(seat.clone());
                Some(self)
            }
            (
                "zwp_tablet_tool_v2",
                zwp_tablet_tool_v2::REQ_SET_CURSOR_OPCODE,
                [Argument::Uint(serial), Argument::Object(surface), Argument::Int(x), Argument::Int(y)],
            ) => {
                state.cursors.push(CursorRequest {
                    tool: msg.sender_id,
                    serial: *serial,
                    surface: surface.clone(),
                    hotspot: [*x, *y],
                });
                None
            }
            _ => panic!("unexpected request: {msg:?}"),
        }
    }

    fn destroyed(
        self: Arc<Self>,
        _: &server::Handle,
        _: &mut Compositor,
        _: server::ClientId,
        _: server::ObjectId,
    ) {
    }
}

#[derive(Default)]
struct Host {
    surfaces: Vec<WlSurface>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Host {
    fn event(
        host: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "wl_compositor" {
                let compositor: WlCompositor = registry.bind(name, 1, qh, ());
                host.surfaces.push(compositor.create_surface(qh, ()));
                host.surfaces.push(compositor.create_surface(qh, ()));
            }
        }
    }
}

wayland_client::delegate_noop!(Host: ignore WlCompositor);
wayland_client::delegate_noop!(Host: ignore WlSurface);

pub(super) struct Window {
    surface: WlSurface,
    pub(super) conn: Connection,
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let ptr = NonNull::new(self.conn.backend().display_ptr().cast()).unwrap();
        // SAFETY: this window owns the connection for the returned handle's lifetime.
        Ok(unsafe { DisplayHandle::borrow_raw(WaylandDisplayHandle::new(ptr).into()) })
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let ptr = NonNull::new(self.surface.id().as_ptr().cast()).unwrap();
        // SAFETY: this window retains the live surface and its connection.
        Ok(unsafe { WindowHandle::borrow_raw(WaylandWindowHandle::new(ptr).into()) })
    }
}

pub(super) struct Fixture {
    // Drop the manager before the host that owns its borrowed display.
    pub(super) manager: crate::Manager,
    pub(super) window: Arc<Window>,
    pub(super) server: server::Backend<Compositor>,
    pub(super) state: Compositor,
    tablet: server::ObjectId,
}

impl Fixture {
    pub(super) fn new(builder: crate::Builder) -> Self {
        let (client, socket) = UnixStream::pair().unwrap();
        let mut server = server::Backend::new().unwrap();
        let mut handle = server.handle();
        handle.insert_client(socket, Arc::new(())).unwrap();
        for interface in [
            WlCompositor::interface(),
            wl_seat::WlSeat::interface(),
            ZwpTabletManagerV2::interface(),
        ] {
            handle.create_global(interface, 1, Arc::new(Object));
        }
        let conn = Connection::from_socket(client).unwrap();
        let mut queue = conn.new_event_queue();
        conn.display().get_registry(&queue.handle(), ());
        let mut state = Compositor::default();
        conn.flush().unwrap();
        server.dispatch_all_clients(&mut state).unwrap();
        server.flush(None).unwrap();
        conn.prepare_read().unwrap().read().unwrap();
        let mut host = Host::default();
        queue.dispatch_pending(&mut host).unwrap();
        conn.flush().unwrap();
        server.dispatch_all_clients(&mut state).unwrap();
        assert_eq!(state.surfaces.len(), 2);

        let window = Arc::new(Window {
            surface: host.surfaces.remove(0),
            conn,
        });
        let mut manager = builder.build_shared(&window).unwrap();
        manager.pump().unwrap();
        server.dispatch_all_clients(&mut state).unwrap();
        server.flush(None).unwrap();
        window.conn.prepare_read().unwrap().read().unwrap();
        manager.pump().unwrap();
        server.dispatch_all_clients(&mut state).unwrap();
        let seat = state.tablet_seat.as_ref().unwrap();
        let tablet = handle
            .create_object(
                handle.get_client(seat.clone()).unwrap(),
                ZwpTabletV2::interface(),
                1,
                Arc::new(Object),
            )
            .unwrap();
        handle
            .send_event(message!(
                seat.clone(),
                zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE,
                [Argument::NewId(tablet.clone())]
            ))
            .unwrap();
        handle
            .send_event(message!(
                tablet.clone(),
                zwp_tablet_v2::EVT_NAME_OPCODE,
                [Argument::Str(Some(Box::new(c"Test tablet".to_owned())))]
            ))
            .unwrap();
        handle
            .send_event(message!(tablet.clone(), zwp_tablet_v2::EVT_DONE_OPCODE, []))
            .unwrap();
        let mut fixture = Self {
            manager,
            window,
            server,
            state,
            tablet,
        };
        fixture.deliver();
        fixture
    }

    pub(super) fn add_tool(&mut self, kind: Option<Type>) -> server::ObjectId {
        let seat = self.state.tablet_seat.as_ref().unwrap();
        let handle = self.server.handle();
        let tool = handle
            .create_object(
                handle.get_client(seat.clone()).unwrap(),
                ZwpTabletToolV2::interface(),
                1,
                Arc::new(Object),
            )
            .unwrap();
        handle
            .send_event(message!(
                seat.clone(),
                zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE,
                [Argument::NewId(tool.clone())]
            ))
            .unwrap();
        if let Some(kind) = kind {
            self.send(
                &tool,
                zwp_tablet_tool_v2::EVT_TYPE_OPCODE,
                &[Argument::Uint(kind as u32)],
            );
        }
        self.send(
            &tool,
            zwp_tablet_tool_v2::EVT_CAPABILITY_OPCODE,
            &[Argument::Uint(
                zwp_tablet_tool_v2::Capability::Pressure as u32,
            )],
        );
        self.send(&tool, zwp_tablet_tool_v2::EVT_DONE_OPCODE, &[]);
        self.deliver();
        tool
    }

    pub(super) fn send(
        &self,
        tool: &server::ObjectId,
        opcode: u16,
        args: &[Argument<server::ObjectId, i32>],
    ) {
        self.server
            .handle()
            .send_event(Message {
                sender_id: tool.clone(),
                opcode,
                args: args.into(),
            })
            .unwrap();
    }

    pub(super) fn enter(&self, tool: &server::ObjectId, serial: u32, surface: usize) {
        self.send(
            tool,
            zwp_tablet_tool_v2::EVT_PROXIMITY_IN_OPCODE,
            &[
                Argument::Uint(serial),
                Argument::Object(self.tablet.clone()),
                Argument::Object(self.state.surfaces[surface].clone()),
            ],
        );
        self.send(
            tool,
            zwp_tablet_tool_v2::EVT_MOTION_OPCODE,
            &[Argument::Fixed(10 * 256), Argument::Fixed(20 * 256)],
        );
    }

    pub(super) fn frame(&self, tool: &server::ObjectId) {
        self.send(
            tool,
            zwp_tablet_tool_v2::EVT_FRAME_OPCODE,
            &[Argument::Uint(1)],
        );
    }

    pub(super) fn deliver(&mut self) -> Vec<&'static str> {
        self.server.flush(None).unwrap();
        // The server has just written an event batch. The host reads the shared
        // display, leaving the manager to dispatch its own private C queue.
        self.window.conn.prepare_read().unwrap().read().unwrap();
        let events = self.manager.pump().unwrap();
        let kinds = events
            .into_iter()
            .filter_map(|event| match event {
                crate::events::Event::Tool { event, .. } => Some(match event {
                    crate::events::ToolEvent::In { .. } => "in",
                    crate::events::ToolEvent::Down => "down",
                    crate::events::ToolEvent::Pose(_) => "pose",
                    crate::events::ToolEvent::Up => "up",
                    crate::events::ToolEvent::Out => "out",
                    crate::events::ToolEvent::Frame(_) => "frame",
                    crate::events::ToolEvent::Removed => "removed",
                    _ => "other",
                }),
                _ => None,
            })
            .collect();
        // No host flush here: a received request proves that pump flushed it.
        self.server.dispatch_all_clients(&mut self.state).unwrap();
        kinds
    }

    pub(super) fn take_cursor(&mut self, tool: &server::ObjectId, serial: u32) {
        let requests = std::mem::take(&mut self.state.cursors);
        assert_eq!(
            requests.len(),
            1,
            "expected one cursor request: {requests:?}"
        );
        let request = &requests[0];
        assert_eq!(&request.tool, tool);
        assert_eq!(request.serial, serial);
        assert!(request.surface.is_null());
        assert_eq!(request.hotspot, [0, 0]);
    }
}
