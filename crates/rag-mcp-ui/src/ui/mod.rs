//! UI panels: canvas, status, detail, empty states, wiki browser.

pub mod canvas;
pub mod detail;
pub mod empty;
pub mod home;
pub mod library;
pub mod operations;
pub mod revisions;
pub mod search;
pub mod status;
pub mod wiki;

/// `ComboBox` 0.36 closes on pointer clicks by default, but keyboard and
/// AccessKit activation do not produce `pointer.any_click()`. Explicitly close
/// the containing popup whenever a choice response activates so every input
/// path has the same one-shot selector behavior.
pub(crate) fn closing_selectable_value<'a, Value: PartialEq>(
    ui: &mut egui::Ui,
    current_value: &mut Value,
    selected_value: Value,
    text: impl egui::IntoAtoms<'a>,
) -> egui::Response {
    let response = ui.selectable_value(current_value, selected_value, text);
    if response.clicked() {
        ui.close();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combo_choice_closes_after_accesskit_activation() {
        let ctx = egui::Context::default();
        let mut selected = "alpha".to_string();
        let mut beta_id = None;

        // Exercise the popup's close contract without coupling the regression
        // to ComboBox popup placement, which intentionally spans layout passes.
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let response = ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("closing_combo_test")
                    .closable(),
                |ui| {
                    beta_id = Some(
                        closing_selectable_value(ui, &mut selected, "beta".to_string(), "Beta").id,
                    );
                },
            );
            assert!(!response.response.should_close());
        });
        output.drop_without_applying_deltas();

        let beta_id = beta_id.expect("choice rendered in closable UI");
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: beta_id.accesskit_id(),
                data: None,
            },
        ));
        let mut close_requested = false;
        let output = ctx.run_ui(input, |ui| {
            let response = ui.scope_builder(
                egui::UiBuilder::new()
                    .id_salt("closing_combo_test")
                    .closable(),
                |ui| {
                    closing_selectable_value(ui, &mut selected, "beta".to_string(), "Beta");
                },
            );
            close_requested = response.response.should_close();
        });
        output.drop_without_applying_deltas();

        assert_eq!(selected, "beta");
        assert!(close_requested);
    }
}
