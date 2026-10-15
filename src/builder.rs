//! Builder-style configuration for connecting to the system tablet API.
//!
//! For a default configuration, `Builder::new().build_{shared, raw}` is all you need!

use crate::{Backing, Manager};

#[derive(thiserror::Error, Debug)]
pub enum BuildError {
    /// The given window handle doesn't use a supported connection type.
    /// This includes cases where the platform is otherwise supported but the feature was disabled at compile-time.
    #[error("handle doesn't contain a supported display type")]
    Unsupported,
    /// Failed to acquire a window handle
    #[error("{0:?}")]
    HandleError(raw_window_handle::HandleError),
    /// The Windows Ink backend could not initialize.
    #[cfg(ink_rts)]
    #[error("Windows Ink initialization failed: {0}")]
    WindowsInk(#[source] windows::core::Error),
}
// #[from] thiserror attribute breaks horribly D:
impl From<raw_window_handle::HandleError> for BuildError {
    fn from(value: raw_window_handle::HandleError) -> Self {
        Self::HandleError(value)
    }
}

/// Pre-construction configuration for a [`Manager`].
// Not every integration will use every field, leaving some "unread" on some targets.
#[allow(unused)]
pub struct Builder {
    pub(crate) emulate_tool_from_mouse: bool,
    pub(crate) hide_tool_cursor: bool,
}
impl Default for Builder {
    fn default() -> Self {
        Self {
            emulate_tool_from_mouse: true,
            hide_tool_cursor: false,
        }
    }
}

/// # Configuration
#[allow(clippy::needless_update)]
impl Builder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Set whether an emulated tablet and tool should be created from mouse input.
    /// This functionality is *not* provided by this crate, but by the system backend.
    ///
    /// Defaults to `true`.
    ///
    /// # Supprted platforms
    /// * Windows Ink
    #[must_use]
    pub fn emulate_tool_from_mouse(self, emulate: bool) -> Self {
        Self {
            emulate_tool_from_mouse: emulate,
            ..self
        }
    }

    /// Hide the compositor's tablet-tool cursor on each proximity entry, including hover.
    ///
    /// Defaults to `false`. Applies to every tablet tool received by this manager:
    /// pens, erasers, brushes, pencils, airbrushes, fingers, lenses, and tablet mice,
    /// including tools whose type is unknown. Ordinary mouse and trackpad cursors
    /// and application-drawn overlays are unaffected.
    ///
    /// The Wayland manager receives tablet events for the shared display, so this
    /// policy also covers sibling surfaces on that connection. It is not restricted
    /// to the window passed to the builder or to a canvas region. It is configured
    /// only at construction; dropping the manager does not restore a cursor.
    ///
    /// # Supported platforms
    /// * Wayland. This option has no effect on Windows Ink.
    #[must_use]
    pub fn hide_tool_cursor(self, hide: bool) -> Self {
        Self {
            hide_tool_cursor: hide,
            ..self
        }
    }
}
/// # Finishing
impl Builder {
    /// Build from a shared display handle carrier. Internally, this `Arc` is kept alive for as
    /// long as the returned `Manager` is around ensuring safe operation.
    // Unimplementable on `rwh_05`, as its safety conditions are not strong enough to ensure this
    // is sound!
    // Silly clippy, it's a self-describing err type!
    #[allow(clippy::missing_errors_doc)]
    pub fn build_shared<Holder>(self, rwh: &std::sync::Arc<Holder>) -> Result<Manager, BuildError>
    where
        Holder: raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle + 'static,
    {
        // Unsize - erase the type, we don't care during runtime. We just need to be able to `Drop` it and to keep it around as
        // long as we need!
        // We can *kind of* skip the clone using an identity `transmute` to extent it's life - the ref remains valid even after unsizing.
        // butttttt the safety is nuanced and may actually be instantaneous UB at the end of scope if `build` returns `Err`. Defeated by borrowchk again!
        let backing = Backing::Arc(rwh.clone() as _);
        // Safety - The returned `display_handle` is valid for as long as `rwh` is due to
        // safety bound on `DisplayHandle::borrow_raw`. Since we keep the `rwh` alive inside the manager,
        // the pointer is thus valid for the lifetime of the manager.
        unsafe { self.build(rwh as &Holder, backing) }
    }
    /// Build from a display handle carrier, such as a reference to a `winit` window, with unbound lifetime.
    ///
    /// # Safety
    /// The given display handle carrier must be keep the window and display pointers valid as long as the returned `Manager` is alive.
    ///
    /// ***`rwh` is dropped at the end of scope** - not kept alive within the `Manager` - thus cannot be used to ensure safety!*
    // Silly clippy, it's a self-describing err type!
    #[allow(clippy::missing_errors_doc)]
    pub unsafe fn build_raw(
        self,
        rwh: impl raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle,
    ) -> Result<Manager, BuildError> {
        // Safety: forwarded to this fn's contract.
        unsafe { self.build(rwh, Backing::Raw) }
    }
    /// Private, raw builder that the others delegate into.
    ///
    /// The `rwh` implementor object is *not* kept.
    /// # Safety
    /// The given display handle carrier must be keep the window and display pointers valid as long as the returned `Manager` is alive.
    /// This may be insured by using the `Backing` parameter which will be kept alive for as long as the returned Manager is.
    #[cfg(any(wl_tablet, ink_rts))]
    unsafe fn build(
        self,
        rwh: impl raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle,
        backing: Backing,
    ) -> Result<Manager, BuildError> {
        let internal = match rwh.display_handle()?.as_raw() {
            #[cfg(wl_tablet)]
            raw_window_handle::RawDisplayHandle::Wayland(wlh) => {
                Ok(crate::platform::PlatformManager::Wayland(
                    // Safety: forwarded to this fn's contract.
                    unsafe {
                        crate::platform::wl::Manager::build_wayland_display(
                            self.hide_tool_cursor,
                            wlh.display.as_ptr().cast(),
                        )
                    },
                ))
            }
            #[cfg(ink_rts)]
            raw_window_handle::RawDisplayHandle::Windows(_) => {
                // We need the window handle for this :V
                // Notably, WinRT is unsupported - It doesn't have the IRealTimeStylus API at all.
                if let raw_window_handle::RawWindowHandle::Win32(wh) = rwh.window_handle()?.as_raw()
                {
                    Ok(crate::platform::PlatformManager::Ink(
                        // Safety: forwarded to this fn's contract.
                        unsafe { crate::platform::ink::Manager::build_hwnd(self, wh.hwnd) }
                            .map_err(BuildError::WindowsInk)?,
                    ))
                } else {
                    Err(BuildError::Unsupported)
                }
            }
            _ => Err(BuildError::Unsupported),
        }?;

        Ok(Manager {
            internal,
            _backing: backing,
        })
    }

    #[cfg(not(any(wl_tablet, ink_rts)))]
    #[expect(
        clippy::unused_self,
        reason = "shares the backend-enabled builder interface"
    )]
    unsafe fn build(
        self,
        _rwh: impl raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle,
        _backing: Backing,
    ) -> Result<Manager, BuildError> {
        Err(BuildError::Unsupported)
    }
}

#[cfg(all(test, not(any(wl_tablet, ink_rts))))]
mod tests {
    use super::*;
    use raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
    };

    struct UnavailableWindow;
    impl HasDisplayHandle for UnavailableWindow {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Err(HandleError::Unavailable)
        }
    }
    impl HasWindowHandle for UnavailableWindow {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            Err(HandleError::Unavailable)
        }
    }

    #[test]
    fn building_without_backends_reports_unsupported() {
        let window = std::sync::Arc::new(UnavailableWindow);
        for builder in [Builder::new(), Builder::new().hide_tool_cursor(true)] {
            assert!(matches!(
                builder.build_shared(&window),
                Err(BuildError::Unsupported)
            ));
        }
    }
}
