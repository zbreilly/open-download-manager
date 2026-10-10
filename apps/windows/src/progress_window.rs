//! Progress.
use crate::app::Shared;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct Input {
    pub shared: Shared,
    pub id: std::rc::Rc<str>,
}

pub struct ProgressWindow;

impl Component for ProgressWindow {
    type Input = Input;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, _input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_frame("Download", TextBlock::new().text("Download"))
    }
}
