//! Site grabber.
use crate::app::Shared;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct Input {
    pub shared: Shared,
    pub links: Option<(u64, std::rc::Rc<serde_json::Value>)>,
}

pub struct GrabberWindow;

impl Component for GrabberWindow {
    type Input = Input;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, _input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_frame("Site Grabber", TextBlock::new().text("Site Grabber"))
    }
}
