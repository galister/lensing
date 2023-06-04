use std::{
    os::{fd::FromRawFd, unix::net::UnixStream},
    sync::{Arc, Mutex},
};

use ashpd::{
    desktop::screencast::{CursorMode, PersistMode, Screencast, SourceType},
    WindowIdentifier,
};
use futures::executor::block_on;
use gstreamer::{
    element_error,
    prelude::{Cast, GstBinExtManual},
    Element, ElementFactory, Pipeline,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::EventLoop,
        client::{
            globals::registry_queue_init,
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
            Connection, QueueHandle, WaylandSource,
        },
    },
    shell::xdg::{
        window::{Window, WindowConfigure, WindowHandler},
        XdgShell,
    },
};
use wl_client_desktop::WlClientDesktopState;

mod pw_capture;
mod wl_client_desktop;

fn main() {

    let wl_desktop = WlClientDesktopState::new();

    for o in wl_desktop.outputs.iter() {
        println!("{}: {} @ {}x{}, offset {}x{}, pixels {}x{}", o.name, o.model, o.logical_size.0, o.logical_size.1, o.logical_pos.0, o.logical_pos.1, o.size.0, o.size.1);
    }
}

// fn wayland() {
//     let connection = Connection::connect_to_env().expect("Unable to connect to wayland");
//     let (globals, event_queue) = registry_queue_init(&connection).unwrap();
//     let qh = event_queue.handle();
//     let mut event_loop: EventLoop<MirrorWindow> =
//         EventLoop::try_new().expect("Failed to initialize the event loop!");
//     let loop_handle = event_loop.handle();
//     WaylandSource::new(event_queue)
//         .unwrap()
//         .insert(loop_handle)
//         .unwrap();

//     let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
//     let xdg_shell = XdgShell::bind(&globals, &qh).expect("xdg shell is not available");
// }

// struct MirrorWindow {}
// impl CompositorHandler for MirrorWindow {
//     fn scale_factor_changed(
//         &mut self,
//         conn: &Connection,
//         qh: &QueueHandle<Self>,
//         surface: &WlSurface,
//         new_factor: i32,
//     ) {
//     }

//     fn frame(&mut self, conn: &Connection, qh: &QueueHandle<Self>, surface: &WlSurface, time: u32) {
//     }
// }
// delegate_compositor!(MirrorWindow);

// impl OutputHandler for MirrorWindow {
//     fn output_state(&mut self) -> &mut OutputState {}

//     fn new_output(&mut self, conn: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {}

//     fn update_output(&mut self, conn: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {}

//     fn output_destroyed(&mut self, conn: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {}
// }
// delegate_output!(MirrorWindow);

// impl WindowHandler for MirrorWindow {
//     fn request_close(&mut self, conn: &Connection, qh: &QueueHandle<Self>, window: &Window) {}

//     fn configure(
//         &mut self,
//         conn: &Connection,
//         qh: &QueueHandle<Self>,
//         window: &Window,
//         configure: WindowConfigure,
//         serial: u32,
//     ) {
//     }
// }
// delegate_xdg_window!(MirrorWindow);
