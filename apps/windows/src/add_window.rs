//! Add Download.
use crate::app::Shared;
use windows_reactor::*;

#[derive(Clone, PartialEq)]
pub struct Input {
    pub shared: Shared,
    pub key: u64,
    pub params: std::rc::Rc<serde_json::Value>,
}

pub struct AddWindow;

impl Component for AddWindow {
    type Input = Input;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, _input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_frame("Add Download", TextBlock::new().text("Add Download"))
    }
}
