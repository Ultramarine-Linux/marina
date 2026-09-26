use crate::errors::Result;

pub trait RenderableSet {
    fn render_all_dirty(&self) -> Result<()>;
}
