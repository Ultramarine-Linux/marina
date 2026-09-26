use slint::LogicalPosition;
use wayland_client::backend::ObjectId;

pub struct PointerInputState {
    active_surface_id: Option<ObjectId>,
    current_position: LogicalPosition,
    accumulated_axis_x: f32,
    accumulated_axis_y: f32,
}

impl PointerInputState {
    pub const fn new() -> Self {
        Self {
            active_surface_id: None,
            current_position: LogicalPosition::new(0.0, 0.0),
            accumulated_axis_x: 0.0,
            accumulated_axis_y: 0.0,
        }
    }

    pub const fn active_surface_id(&self) -> Option<&ObjectId> {
        self.active_surface_id.as_ref()
    }

    pub fn set_active_surface(&mut self, surface_id: Option<ObjectId>) {
        self.active_surface_id = surface_id;
    }

    pub const fn current_position(&self) -> LogicalPosition {
        self.current_position
    }

    pub fn set_current_position(&mut self, position: LogicalPosition) {
        self.current_position = position;
    }

    pub fn accumulate_axis_value(&mut self, delta_x: f32, delta_y: f32) {
        self.accumulated_axis_x += delta_x;
        self.accumulated_axis_y += delta_y;
    }

    pub fn take_accumulated_axis(&mut self) -> (f32, f32) {
        let delta_x = self.accumulated_axis_x;
        let delta_y = self.accumulated_axis_y;
        self.accumulated_axis_x = 0.0;
        self.accumulated_axis_y = 0.0;
        (delta_x, delta_y)
    }

    pub fn reset(&mut self) {
        self.active_surface_id = None;
        self.current_position = LogicalPosition::new(0.0, 0.0);
        self.accumulated_axis_x = 0.0;
        self.accumulated_axis_y = 0.0;
    }

    pub fn clear_surface_if_matches(&mut self, surface_id: &ObjectId) {
        if self.active_surface_id.as_ref() == Some(surface_id) {
            self.active_surface_id = None;
        }
    }

    pub const fn has_active_surface(&self) -> bool {
        self.active_surface_id.is_some()
    }
}

impl Default for PointerInputState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct KeyboardInputState {
    focused_surface_id: Option<ObjectId>,
}

impl KeyboardInputState {
    pub const fn new() -> Self {
        Self {
            focused_surface_id: None,
        }
    }

    pub const fn focused_surface_id(&self) -> Option<&ObjectId> {
        self.focused_surface_id.as_ref()
    }

    pub fn set_focused_surface(&mut self, surface_id: Option<ObjectId>) {
        self.focused_surface_id = surface_id;
    }

    pub fn reset(&mut self) {
        self.focused_surface_id = None;
    }

    pub fn clear_surface_if_matches(&mut self, surface_id: &ObjectId) {
        if self.focused_surface_id.as_ref() == Some(surface_id) {
            self.focused_surface_id = None;
        }
    }

    pub const fn has_focused_surface(&self) -> bool {
        self.focused_surface_id.is_some()
    }
}

impl Default for KeyboardInputState {
    fn default() -> Self {
        Self::new()
    }
}
