//! Native PipeWire volume controls for Marina.

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VolumeState {
    pub percent: u8,
    pub muted: bool,
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("PipeWire operation failed: {0}")]
    PipeWire(String),
    #[error("no PipeWire audio sink is available")]
    NoSink,
}

mod backend {
    use std::{cell::RefCell, io::Cursor, rc::Rc, time::Duration};

    use pipewire as pw;
    use pw::{
        node::{Node, NodeListener},
        spa::{
            param::ParamType,
            pod::{
                Object, Pod, Property, PropertyFlags, Value, ValueArray,
                deserialize::PodDeserializer, serialize::PodSerializer,
            },
            utils::SpaTypes,
        },
        types::ObjectType,
    };

    use super::{AudioError, VolumeState};

    struct SinkProxy {
        _node: Node,
        _listener: Option<NodeListener>,
    }

    pub fn volume() -> Result<VolumeState, AudioError> {
        pw::init();
        let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
        let context = pw::context::ContextRc::new(&main_loop, None).map_err(error)?;
        let core = context.connect_rc(None).map_err(error)?;
        let registry = core.get_registry_rc().map_err(error)?;
        let result = Rc::new(RefCell::new(None));
        let sink = Rc::new(RefCell::new(None::<SinkProxy>));

        let registry_weak = registry.downgrade();
        let main_loop_for_global = main_loop.clone();
        let result_for_global = result.clone();
        let sink_for_global = sink.clone();
        let timeout_loop = main_loop.clone();
        let timeout = main_loop.loop_().add_timer(move |_| timeout_loop.quit());
        let _ = timeout.update_timer(Some(Duration::from_secs(1)), None);
        let _registry_listener = registry
            .add_listener_local()
            .global(move |object| {
                if sink_for_global.borrow().is_some() || !is_audio_sink(object) {
                    return;
                }
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                let Ok(node) = registry.bind::<Node, _>(object) else {
                    return;
                };
                let result = result_for_global.clone();
                let loop_weak = main_loop_for_global.downgrade();
                let listener = node
                    .add_listener_local()
                    .param(move |_seq, id, _index, _next, param| {
                        if id != ParamType::Props {
                            return;
                        }
                        if let Some(param) = param {
                            if let Some(state) = volume_from_pod(param) {
                                *result.borrow_mut() = Some(state);
                            }
                        }
                        if let Some(main_loop) = loop_weak.upgrade() {
                            main_loop.quit();
                        }
                    })
                    .register();
                node.enum_params(0, Some(ParamType::Props), 0, u32::MAX);
                *sink_for_global.borrow_mut() = Some(SinkProxy {
                    _node: node,
                    _listener: Some(listener),
                });
            })
            .register();

        main_loop.run();
        result.borrow().to_owned().ok_or(AudioError::NoSink)
    }

    pub fn set_volume(percent: u8) -> Result<(), AudioError> {
        pw::init();
        let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
        let context = pw::context::ContextRc::new(&main_loop, None).map_err(error)?;
        let core = context.connect_rc(None).map_err(error)?;
        let registry = core.get_registry_rc().map_err(error)?;
        let sink = Rc::new(RefCell::new(None::<SinkProxy>));
        let applied = Rc::new(RefCell::new(false));

        let registry_weak = registry.downgrade();
        let loop_weak = main_loop.downgrade();
        let sink_for_global = sink.clone();
        let applied_for_global = applied.clone();
        let timeout_loop = main_loop.clone();
        let timeout = main_loop.loop_().add_timer(move |_| timeout_loop.quit());
        let _ = timeout.update_timer(Some(Duration::from_secs(1)), None);
        let _registry_listener = registry
            .add_listener_local()
            .global(move |object| {
                if sink_for_global.borrow().is_some() || !is_audio_sink(object) {
                    return;
                }
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                let Ok(node) = registry.bind::<Node, _>(object) else {
                    return;
                };
                if let Ok(bytes) = volume_pod(percent) {
                    if let Some(pod) = Pod::from_bytes(&bytes) {
                        node.set_param(ParamType::Props, 0, pod);
                        *applied_for_global.borrow_mut() = true;
                    }
                }
                *sink_for_global.borrow_mut() = Some(SinkProxy {
                    _node: node,
                    _listener: None,
                });
                if let Some(main_loop) = loop_weak.upgrade() {
                    main_loop.quit();
                }
            })
            .register();

        main_loop.run();
        if *applied.borrow() {
            Ok(())
        } else {
            Err(AudioError::NoSink)
        }
    }

    fn is_audio_sink(object: &pw::registry::GlobalObject<&pw::spa::utils::dict::DictRef>) -> bool {
        object.type_ == ObjectType::Node
            && object
                .props
                .and_then(|props| props.get(*pw::keys::MEDIA_CLASS))
                .is_some_and(|class| class == "Audio/Sink")
    }

    fn volume_from_pod(pod: &Pod) -> Option<VolumeState> {
        let (_, value) = PodDeserializer::deserialize_any_from(pod.as_bytes()).ok()?;
        let Value::Object(object) = value else {
            return None;
        };
        let mut volume = None;
        let mut muted = false;
        for property in object.properties {
            if property.key == pw::spa::sys::SPA_PROP_volume {
                if let Value::Float(value) = property.value {
                    volume = Some(value);
                }
            } else if property.key == pw::spa::sys::SPA_PROP_channelVolumes {
                if let Value::ValueArray(ValueArray::Float(values)) = property.value {
                    if let Some(value) = values.first().copied() {
                        volume = Some(value);
                    }
                }
            } else if property.key == pw::spa::sys::SPA_PROP_mute {
                if let Value::Bool(value) = property.value {
                    muted = value;
                }
            }
        }
        Some(VolumeState {
            percent: (volume? * 100.0).round().clamp(0.0, 100.0) as u8,
            muted,
        })
    }

    fn volume_pod(percent: u8) -> Result<Vec<u8>, AudioError> {
        let volume = f32::from(percent.min(100)) / 100.0;
        let object = Object {
            type_: SpaTypes::ObjectParamProps.as_raw(),
            id: ParamType::Props.as_raw(),
            properties: vec![Property {
                key: pw::spa::sys::SPA_PROP_volume,
                flags: PropertyFlags::empty(),
                value: Value::Float(volume),
            }],
        };
        PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
            .map(|success| success.0.into_inner())
            .map_err(|error| AudioError::PipeWire(error.to_string()))
    }

    fn error(error: pw::Error) -> AudioError {
        AudioError::PipeWire(error.to_string())
    }
}

pub use backend::{set_volume, volume};
