//! Settings.
use crate::app::Shared;
use windows_reactor::*;


pub struct SettingsWindow;

impl Component for SettingsWindow {
    type Input = Shared;
    type Message = ();

    fn create(_: &Self::Input, _: &ComponentContext<Self>) -> Self {
        Self
    }

    fn view(&self, _input: &Self::Input, context: &mut ViewContext<Self>) -> View {
        context.window_frame("Settings", TextBlock::new().text("Settings"))
    }
}
