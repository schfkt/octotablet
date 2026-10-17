//! Regressions for the rejected dedicated-reader design; no production worker.
use super::{super::Manager, fixture::Fixture};
use std::{
    io::ErrorKind,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    task::{Context, Wake, Waker},
    thread,
    time::{Duration, Instant},
};
use wayland_backend::client::WaylandError;
use wayland_client::{protocol::wl_callback, Connection, Dispatch, QueueHandle};

use rustix::event::{poll, PollFd, PollFlags};

fn readable(guard: &wayland_backend::client::ReadEventsGuard) -> bool {
    let mut fds = [PollFd::from_borrowed_fd(
        guard.connection_fd(),
        PollFlags::IN,
    )];
    poll(&mut fds, 0).unwrap();
    fds[0].revents().contains(PollFlags::IN)
}

const WAIT: Duration = Duration::from_secs(2);

// A regression in libwayland read coordination must fail a test, never hang the
// test runner. Every experiment (including fixture startup) runs in a child.
fn isolated(name: &str, experiment: impl FnOnce()) {
    const CHILD: &str = "OCTOTABLET_WAKE_EXPERIMENT";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        experiment();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("platform::wl::tests::wake::{name}"),
            "--nocapture",
        ])
        .env(CHILD, name)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{name}: child failed: {status}");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{name}: experiment exceeded 15 seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wayland(fixture: &mut Fixture) -> &mut Manager {
    match &mut fixture.manager.internal {
        crate::platform::PlatformManager::Wayland(manager) => manager,
    }
}

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn dispatch_and_arm(manager: &mut Manager, waker: &Waker) {
    assert!(manager
        .queue
        .poll_dispatch_pending(&mut Context::from_waker(waker), &mut manager.state,)
        .is_pending());
}

fn empty_read(result: Result<usize, WaylandError>) {
    match result {
        Ok(0) => {}
        Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {}
        other => panic!("expected no readable events, got {other:?}"),
    }
}

#[test]
fn host_read_needs_private_dispatch_before_rust_queue_wakes() {
    isolated(
        "host_read_needs_private_dispatch_before_rust_queue_wakes",
        || {
            let mut fixture = Fixture::new(crate::Builder::new());
            let pen = fixture.add_tool(None);
            wayland(&mut fixture).state.cleanup_start();
            let notifications = Arc::new(Wakes::default());
            let waker = Waker::from(notifications.clone());
            dispatch_and_arm(wayland(&mut fixture), &waker);
            fixture.enter(&pen, 1, 0);
            fixture.frame(&pen);
            fixture.frame(&pen);
            fixture.server.flush(None).unwrap();
            fixture.window.conn.prepare_read().unwrap().read().unwrap();

            assert_eq!(notifications.0.load(Ordering::SeqCst), 0);
            assert!(wayland(&mut fixture).state.events.is_empty());
            assert!(wayland(&mut fixture).conn.prepare_read().is_none());
            // The host drained the socket, while the guest C queue still holds input.
            let host_guard = fixture.window.conn.prepare_read().unwrap();
            assert!(!readable(&host_guard));
            empty_read(host_guard.read());
            let backend = wayland(&mut fixture).conn.backend();
            let owner = fixture.window.clone();
            thread::spawn(move || {
                let _owner = owner;
                assert!(backend.dispatch_inner_queue().unwrap() > 0);
            })
            .join()
            .unwrap();
            assert!(notifications.0.load(Ordering::SeqCst) > 0);
            // ObjectData enqueued events; typed TabletState callbacks are still idle.
            assert!(wayland(&mut fixture).state.events.is_empty());
            dispatch_and_arm(wayland(&mut fixture), &waker);
            assert_eq!(
                wayland(&mut fixture)
                    .state
                    .events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        crate::events::raw::Event::Tool {
                            event: crate::events::raw::ToolEvent::Frame(_),
                            ..
                        }
                    ))
                    .count(),
                2
            );
        },
    );
}

#[test]
fn prepared_read_can_cancel_locally_with_stalled_host_and_compositor() {
    isolated(
        "prepared_read_can_cancel_locally_with_stalled_host_and_compositor",
        || {
            for _ in 0..8 {
                let mut fixture = Fixture::new(crate::Builder::new());
                let host_guard = fixture.window.conn.prepare_read().unwrap();
                let backend = wayland(&mut fixture).conn.backend();
                let owner = fixture.window.clone();
                let (ready_tx, ready_rx) = mpsc::channel();
                let (stop_tx, stop_rx) = mpsc::channel();
                let worker = thread::spawn(move || {
                    let _owner = owner;
                    let guard = backend.prepare_read().unwrap();
                    ready_tx.send(()).unwrap();
                    stop_rx.recv_timeout(WAIT).unwrap();
                    drop(guard);
                });
                ready_rx.recv_timeout(WAIT).unwrap();
                stop_tx.send(()).unwrap();
                worker.join().unwrap();
                // Neither a host read/cancel nor any server response was needed.
                drop(host_guard);
            }
        },
    );
}

#[test]
fn consumed_read_waits_for_host_guard_despite_local_stop() {
    isolated(
        "consumed_read_waits_for_host_guard_despite_local_stop",
        || {
            let mut fixture = Fixture::new(crate::Builder::new());
            let pen = fixture.add_tool(None);
            let host_guard = fixture.window.conn.prepare_read().unwrap();
            let backend = wayland(&mut fixture).conn.backend();
            let owner = fixture.window.clone();
            let owner_count = Arc::strong_count(&fixture.window);
            let (ready_tx, ready_rx) = mpsc::channel();
            let (go_tx, go_rx) = mpsc::channel();
            let (stop_tx, stop_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                let _owner = owner;
                let guard = backend.prepare_read().unwrap();
                ready_tx.send(()).unwrap();
                go_rx.recv_timeout(WAIT).unwrap();
                ready_tx.send(()).unwrap();
                let result = guard.read();
                done_tx.send((result, stop_rx.try_recv())).unwrap();
            });
            ready_rx.recv_timeout(WAIT).unwrap();
            fixture.frame(&pen);
            fixture.server.flush(None).unwrap();
            assert!(readable(&host_guard));
            go_tx.send(()).unwrap();
            ready_rx.recv_timeout(WAIT).unwrap();
            stop_tx.send(()).unwrap();
            // Data is ready, and cancellation is queued. The held host preparation
            // still prevents read() returning. Its consumed guard cannot be dropped.
            assert!(matches!(
                done_rx.recv_timeout(Duration::from_millis(200)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            drop(host_guard);
            let (result, stopped) = done_rx.recv_timeout(WAIT).unwrap();
            assert!(result.is_ok());
            assert_eq!(stopped, Ok(()));
            worker.join().unwrap();
            assert_eq!(Arc::strong_count(&fixture.window), owner_count - 1);
        },
    );
}

#[derive(Default)]
struct HostPulse(bool);
impl Dispatch<wl_callback::WlCallback, ()> for HostPulse {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.0 = true;
    }
}

#[test]
fn unrelated_host_traffic_does_not_wake_tablet_queue() {
    isolated("unrelated_host_traffic_does_not_wake_tablet_queue", || {
        let mut fixture = Fixture::new(crate::Builder::new());
        let notifications = Arc::new(Wakes::default());
        dispatch_and_arm(wayland(&mut fixture), &Waker::from(notifications.clone()));
        let mut host_queue = fixture.window.conn.new_event_queue::<HostPulse>();
        fixture.window.conn.display().sync(&host_queue.handle(), ());
        fixture.window.conn.flush().unwrap();
        fixture
            .server
            .dispatch_all_clients(&mut fixture.state)
            .unwrap();
        fixture.server.flush(None).unwrap();
        let backend = wayland(&mut fixture).conn.backend();
        backend.prepare_read().unwrap().read().unwrap();
        assert_eq!(backend.dispatch_inner_queue().unwrap(), 0);
        let guard = backend.prepare_read().unwrap();
        assert!(!readable(&guard));
        empty_read(guard.read());
        assert_eq!(notifications.0.load(Ordering::SeqCst), 0);
        let mut host = HostPulse::default();
        host_queue.dispatch_pending(&mut host).unwrap();
        assert!(host.0);
    });
}

#[test]
fn reader_observes_compositor_disconnect() {
    isolated("reader_observes_compositor_disconnect", || {
        let mut fixture = Fixture::new(crate::Builder::new());
        let backend = wayland(&mut fixture).conn.backend();
        let guard = backend.prepare_read().unwrap();
        drop(fixture.server);
        assert!(
            matches!(guard.read(), Err(WaylandError::Io(error)) if error.kind() != ErrorKind::WouldBlock)
        );
    });
}
