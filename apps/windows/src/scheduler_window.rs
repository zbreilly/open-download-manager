//! Queues and scheduler.
use crate::app::Shared;
use windows_reactor::*;


pub struct SchedulerWindow;

impl Component for SchedulerWindow {
    type Input = Shared;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, _input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_frame("Queues & Scheduler", TextBlock::new().text("Queues & Scheduler"))
    }
}
