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

fn main() {
    let proxy = block_on(Screencast::new()).unwrap();
    let session = block_on(proxy.create_session()).unwrap();
    block_on(proxy.select_sources(
        &session,
        CursorMode::Embedded,
        SourceType::Monitor | SourceType::Virtual,
        false,
        None,
        PersistMode::Application,
    ))
    .unwrap();

    let stream = block_on(proxy.start(&session, &WindowIdentifier::None))
        .unwrap()
        .response()
        .unwrap()
        .streams()
        .first()
        .unwrap();
    let pw_fd = block_on(proxy.open_pipe_wire_remote(&session)).unwrap();

    let pipeline = Pipeline::default();
    let src = ElementFactory::make("pipewiresink")
        .property("num-buffers", 250i32)
        .property("fd", pw_fd)
        .build()
        .unwrap();
    let sink = ElementFactory::make("appsink").build().unwrap();
    pipeline.add_many(&[&src, &sink]).unwrap();
    Element::link_many(&[&src, &sink]).unwrap();
    let appsink = sink
        .downcast::<gstreamer_app::AppSink>()
        .expect("is not a appsink");

    let stream = Arc::new(Mutex::new(unsafe { UnixStream::from_raw_fd(pw_fd) }));

    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            // Add a handler to the "eos" signal
            .eos({
                let stream = stream.clone();
                move |_| {
                    // Close the stream part of the UnixSocket pair, this will automatically
                    // create a eos in the receiving part.
                    let _ = stream.lock().unwrap().shutdown(std::net::Shutdown::Write);
                }
            })
            // Add a handler to the "new-sample" signal.
            .new_sample(move |appsink| {
                // Pull the sample in question out of the appsink's buffer.
                let sample = appsink
                    .pull_sample()
                    .map_err(|_| gstreamer::FlowError::Eos)?;
                let buffer = sample.buffer().ok_or_else(|| {
                    element_error!(
                        appsink,
                        gstreamer::ResourceError::Failed,
                        ("Failed to get buffer from appsink")
                    );

                    gstreamer::FlowError::Error
                })?;

                if buffer.n_memory() != 1 {
                    element_error!(
                        appsink,
                        gstreamer::ResourceError::Failed,
                        ("Expected buffer with single memory")
                    );

                    return Err(gstreamer::FlowError::Error);
                }

                let mem = buffer.peek_memory(0);

                // We can use downcast_memory_ref to check if the provided
                // memory is allocated by FdMemoryAllocator or a subtype of it.
                // Note: This is not used in the example, we will always copy
                // the memory to a new shared memory file.
                if let Some(fd_memory) = mem.downcast_memory_ref::<gstreamer_allocators::FdMemory>()
                {
                    // As we already got a fd we can just directly send it over the socket.
                    // NOTE: Synchronization is left out of this example, in a real world
                    // application access to the memory should be synchronized.
                    // For example wayland provides a release callback to signal that
                    // the memory is no longer in use.
                    stream
                        .lock()
                        .unwrap()
                        .send_fds(&[0u8; 1], &[fd_memory.fd()])
                        .map_err(|_| {
                            element_error!(
                                appsink,
                                gstreamer::ResourceError::Failed,
                                ("Failed to send fd over unix stream")
                            );

                            gstreamer::FlowError::Error
                        })?;
                } else {
                    // At this point, buffer is only a reference to an existing memory region somewhere.
                    // When we want to access its content, we have to map it while requesting the required
                    // mode of access (read, read/write).
                    // This type of abstraction is necessary, because the buffer in question might not be
                    // on the machine's main memory itself, but rather in the GPU's memory.
                    // So mapping the buffer makes the underlying memory region accessible to us.
                    // See: https://gstreamerreamer.freedesktop.org/documentation/plugin-development/advanced/allocation.html
                    let map = buffer.map_readable().map_err(|_| {
                        element_error!(
                            appsink,
                            gstreamer::ResourceError::Failed,
                            ("Failed to map buffer readable")
                        );

                        gstreamer::FlowError::Error
                    })?;

                    // Note: To simplify this example we always create a new shared memory file instead
                    // of using a pool of buffers. When using a pool we need to make sure access to the
                    // file is synchronized.
                    let opts = memfd::MemfdOptions::default().allow_sealing(true);
                    let mfd = opts.create("gstreamer-examples").map_err(|err| {
                        element_error!(
                            appsink,
                            gstreamer::ResourceError::Failed,
                            ("Failed to allocated fd: {}", err)
                        );

                        gstreamer::FlowError::Error
                    })?;

                    mfd.as_file().set_len(map.size() as u64).map_err(|err| {
                        element_error!(
                            appsink,
                            gstreamer::ResourceError::Failed,
                            ("Failed to resize fd memory: {}", err)
                        );

                        gstreamer::FlowError::Error
                    })?;

                    let mut seals = memfd::SealsHashSet::new();
                    seals.insert(memfd::FileSeal::SealShrink);
                    seals.insert(memfd::FileSeal::SealGrow);
                    mfd.add_seals(&seals).map_err(|err| {
                        element_error!(
                            appsink,
                            gstreamer::ResourceError::Failed,
                            ("Failed to add fd seals: {}", err)
                        );

                        gstreamer::FlowError::Error
                    })?;

                    mfd.add_seal(memfd::FileSeal::SealSeal).map_err(|err| {
                        element_error!(
                            appsink,
                            gstreamer::ResourceError::Failed,
                            ("Failed to add fd seals: {}", err)
                        );

                        gstreamer::FlowError::Error
                    })?;

                    unsafe {
                        let mut mmap = MmapMut::map_mut(mfd.as_file()).map_err(|_| {
                            element_error!(
                                appsink,
                                gstreamer::ResourceError::Failed,
                                ("Failed to mmap fd")
                            );

                            gstreamer::FlowError::Error
                        })?;

                        mmap.copy_from_slice(map.as_slice());
                    };

                    stream
                        .lock()
                        .unwrap()
                        .send_fds(&[0u8; 1], &[mfd.as_raw_fd()])
                        .map_err(|_| {
                            element_error!(
                                appsink,
                                gstreamer::ResourceError::Failed,
                                ("Failed to send fd over unix stream")
                            );

                            gstreamer::FlowError::Error
                        })?;
                };

                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    // wayland();
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
