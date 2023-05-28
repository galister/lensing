use std::io::Cursor;
use std::ptr::NonNull;

use libspa_sys::spa_pod;
use pipewire::prelude::*;
use pipewire::properties;
use pipewire::spa::pod::deserialize::PodDeserializer;
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{ChoiceValue, Object, Property, PropertyFlags, Value};
use pipewire::spa::utils::{Choice, ChoiceFlags, Fraction, Rectangle};
use pipewire::spa::utils::{ChoiceEnum, Id};
use pipewire::stream::{Stream, StreamFlags};
use pipewire::{Context, Error, MainLoop};

#[derive(Debug, Clone, Copy)]
struct PipewireFrameFormat {
    width: u32,
    height: u32,
    format: u32,
    modifier: u64,
}

impl PipewireFrameFormat {
    fn modifier_hi(&self) -> u32 {
        (self.modifier >> 32) as _
    }
    fn modifier_lo(&self) -> u32 {
        (self.modifier & 0xFFFFFFFF) as _
    }
}

#[derive(Debug, Clone, Copy)]
struct PipewireDmabufPlane {
    fd: i32,
    offset: u32,
    stride: i32,
}

#[derive(Debug, Clone, Copy)]
struct DrmFormat {
    code: u32,
    modifier: u64,
}

fn fourcc_to_spa_video_format(fourcc: u32) -> Option<u32> 
{
    match fourcc {
        //DRM_FORMAT_ARGB8888 (order on fourcc are reversed ARGB = BGRA)
        0x34325241 => Some(libspa_sys::SPA_VIDEO_FORMAT_BGRA), 
        //DRM_FORMAT_ABGR8888
        0x34324241 => Some(libspa_sys::SPA_VIDEO_FORMAT_RGBA), 
        //DRM_FORMAT_XRGB8888
        0x34325258 => Some(libspa_sys::SPA_VIDEO_FORMAT_BGRx), 
        //DRM_FORMAT_XBGR8888
        0x34324258 => Some(libspa_sys::SPA_VIDEO_FORMAT_RGBx), 
        _ => None
    }
}

fn format_dmabuf_params() -> Vec<u8> 
{
    let pod = Value::Object(Object {
        type_: libspa_sys::SPA_TYPE_OBJECT_ParamBuffers,
        id: libspa_sys::SPA_PARAM_Buffers,
        properties: vec![
            Property {
                key: libspa_sys::SPA_PARAM_BUFFERS_dataType,
                flags: PropertyFlags::empty(),
                value: Value::Id(Id(libspa_sys::SPA_DATA_DmaBuf)),
            },
        ],
    });
    let (c, _) = PodSerializer::serialize(Cursor::new(Vec::new()), &pod).unwrap();
    c.into_inner()
}

fn format_get_params(format: u32, modifier: u64, fps: u32) -> Vec<u8> {
    let pod = Value::Object(Object {
        type_: libspa_sys::SPA_TYPE_OBJECT_Format,
        id: libspa_sys::SPA_PARAM_EnumFormat,
        properties: vec![
            Property {
                key: libspa_sys::SPA_FORMAT_mediaType,
                flags: PropertyFlags::empty(),
                value: Value::Id(Id(libspa_sys::SPA_MEDIA_TYPE_video)),
            },
            Property {
                key: libspa_sys::SPA_FORMAT_mediaSubtype,
                flags: PropertyFlags::empty(),
                value: Value::Id(Id(libspa_sys::SPA_MEDIA_SUBTYPE_raw)),
            },
            Property {
                key: libspa_sys::SPA_FORMAT_VIDEO_format,
                flags: PropertyFlags::empty(),
                value: Value::Id(Id(format)),
            },
            Property {
                key: libspa_sys::SPA_FORMAT_VIDEO_modifier,
                flags: PropertyFlags::MANDATORY | PropertyFlags::DONT_FIXATE,
                value: Value::Id(Id(modifier as _)),
            },
            Property {
                key: libspa_sys::SPA_FORMAT_VIDEO_size,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Rectangle(Choice(
                    ChoiceFlags { bits: 0 },
                    ChoiceEnum::Range {
                        default: Rectangle {
                            width: 256,
                            height: 256,
                        },
                        min: Rectangle {
                            width: 1,
                            height: 1,
                        },
                        max: Rectangle {
                            width: 8192,
                            height: 8192,
                        },
                    },
                ))),
            },
            Property {
                key: libspa_sys::SPA_FORMAT_VIDEO_framerate,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Fraction(Choice(
                    ChoiceFlags { bits: 0 },
                    ChoiceEnum::Range {
                        default: Fraction { num: fps, denom: 1 },
                        min: Fraction { num: 0, denom: 1 },
                        max: Fraction {
                            num: 1000,
                            denom: 1,
                        },
                    },
                ))),
            },
        ],
    });

    let (c, _) = PodSerializer::serialize(Cursor::new(Vec::new()), &pod).unwrap();
    c.into_inner()
}

fn pipewire_init_stream<F>(name: &str, node_id: u32, fps: u32, formats: Vec<DrmFormat>, on_frame: F) -> Result<(), Error>
where
    F: Fn(&PipewireFrameFormat, &Vec<PipewireDmabufPlane>),
{
    let main_loop = MainLoop::new()?;
    let context = Context::new(&main_loop)?;
    let core = context.connect(None)?;

    let mut format = PipewireFrameFormat { width: 0, height: 0, format: 0, modifier: 0 };

    let mut stream = Stream::<i32>::with_user_data(
        &main_loop,
        name,
        properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
        0,
    )
    .param_changed(|_, id, param| {
        if param.is_null() || *id != libspa_sys::SPA_PARAM_Format as _ {
            return;
        } 

        let ptr : NonNull<spa_pod> = NonNull::new(param as *mut _).unwrap();
        let pod = unsafe { PodDeserializer::deserialize_ptr(ptr) };
        
        // TODO read format from pod
        // Usually done via spa_format_video_raw_parse

        format.width = 0; // format.info.raw.size.width
        format.height = 0; // format.info.raw.size.height
        format.format = 0; // format.info.raw.format
        format.modifier = 0; // format.info.raw.modifier
        
        let params = format_dmabuf_params();
        // TODO make stream available in here
        stream.update_params(&mut [params.as_ptr() as _]);
    })
    .state_changed(|old, new| {
        println!("Stream state changed: {:?} -> {:?}", old, new);
    })
    .process(|stream, _| {
        let maybe_buffer = None;
        // discard all but the freshest ingredients
        while let Some(buffer) = stream.dequeue_buffer() {
            maybe_buffer = Some(buffer);
        }

        if let Some(buffer) = maybe_buffer {
            let datas = buffer.datas_mut();
            if datas.len() < 0 {
                return;
            }
            let planes: Vec<PipewireDmabufPlane> = datas
                .iter()
                .map(|p| PipewireDmabufPlane {
                    fd: 0, // TODO https://gitlab.freedesktop.org/pipewire/pipewire-rs/-/blob/main/libspa/src/data.rs#L70
                    offset: p.chunk().offset(),
                    stride: p.chunk().stride(),
                })
                .collect();
            on_frame(&format, &planes);
        }
    })
    .create()?;

    let format_params: Vec<*const spa_pod> = formats.iter().filter_map(|f| {
        let spa_video_format = fourcc_to_spa_video_format(f.code)?;
        Some(format_get_params(spa_video_format, f.modifier, fps).as_ptr() as _)
    }).collect();

    stream.connect(
        pipewire::spa::Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        format_params.as_mut_slice(),
    );

    main_loop.run();
    unsafe { pipewire::deinit() };

    Ok(())
}
