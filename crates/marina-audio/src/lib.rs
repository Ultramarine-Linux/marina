//! PipeWire volume controls for Marina.

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
    use std::{cell::RefCell, process::Command, rc::Rc, time::Duration};

    use pipewire as pw;
    use pw::{
        node::{Node, NodeListener},
        spa::{
            param::ParamType,
            pod::{Pod, Value, ValueArray, deserialize::PodDeserializer},
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
        let volume = format!("{}%", percent.min(100));
        let output = Command::new("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &volume])
            .output()
            .map_err(|error| AudioError::PipeWire(format!("failed to run wpctl: {error}")))?;

        if output.status.success() {
            return Ok(());
        }

        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(AudioError::PipeWire(if message.is_empty() {
            format!("wpctl exited with {}", output.status)
        } else {
            message
        }))
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
            percent: (raw_volume_to_normalized(volume?) * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8,
            muted,
        })
    }

    // SPA stores volume on a cubic scale, while WirePlumber tools such as wpctl
    // expose the cube root as the user-facing normalized volume.
    fn raw_volume_to_normalized(volume: f32) -> f32 {
        volume.max(0.0).cbrt()
    }

    fn error(error: pw::Error) -> AudioError {
        AudioError::PipeWire(error.to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::raw_volume_to_normalized;

        #[test]
        fn converts_pipewire_cubic_volume_to_normalized_scale() {
            assert!((raw_volume_to_normalized(0.125) - 0.5).abs() < f32::EPSILON);
            assert!((raw_volume_to_normalized(0.512) - 0.8).abs() < f32::EPSILON);
        }

        #[test]
        fn volume_scale_conversion_clamps_negative_values() {
            assert_eq!(raw_volume_to_normalized(-0.1), 0.0);
        }
    }
}

pub use backend::{set_volume, volume};
