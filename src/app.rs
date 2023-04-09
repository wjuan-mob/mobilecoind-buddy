use crate::{Config, TokenId, TokenInfo, Worker};
use egui::{Align, Button, CentralPanel, Grid, Layout, TopBottomPanel};
use rust_decimal::{prelude::*, Decimal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

/// The three panels the app can show
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
enum Mode {
    #[default]
    Assets,
    Send,
    Swap,
}

/// The App implements eframe::App and is called frequently to redraw the state,
/// it also receives user interaction.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct App {
    /// Which panel we are rendering right now
    mode: Mode,
    /// Which token we most recently selected to send
    send_token_id: TokenId,
    /// Which quantity we most recently selected to send (per token id)
    send_value: HashMap<TokenId, String>,
    /// Which public address we most recently selected to send to
    send_to: String,
    /// Which token we most recently selected to swap from
    swap_from_token_id: TokenId,
    /// Which token we most recently selected to swap to
    swap_to_token_id: TokenId,
    /// Which token value which most recently selected to swap for (per swap_to_token_id)
    swap_to_value: HashMap<TokenId, String>,
    /// The worker is doing balance checking with mobilecoind in the background
    #[serde(skip)]
    worker: Option<Arc<Worker>>,
}

// TokenId does not implement default so we have to do this manually
impl Default for App {
    fn default() -> App {
        App {
            mode: Default::default(),
            send_token_id: TokenId::from(0),
            send_value: Default::default(),
            send_to: Default::default(),
            swap_from_token_id: TokenId::from(0),
            swap_to_token_id: TokenId::from(1),
            swap_to_value: Default::default(),
            worker: None,
        }
    }
}

impl App {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>, _config: Config, worker: Arc<Worker>) -> Self {
        // This is also where you can customize the look and feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        let mut result = if let Some(storage) = cc.storage {
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            App::default()
        };

        result.worker = Some(worker);
        result
    }
}

impl eframe::App for App {
    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let worker = self
            .worker
            .as_mut()
            .expect("intialization failed, no worker is present");

        ctx.set_pixels_per_point(5.0);

        // The top panel is always shown no matter what mode we are in,
        // it shows the public address and sync %
        TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                let public_address = worker.get_b58_address();
                let sync_percent = worker.get_sync_percent();
                // Add a display of the public address and the sync %
                ui.label(format!("Public address: {public_address}"));
                ui.label(format!("Ledger sync: {sync_percent}%"));

                egui::warn_if_debug_build(ui);

                // Check if the worker has reported any error, if so, show it
                ui.horizontal(|ui| {
                    if let Some(err_str) = worker.top_error() {
                        ui.label(err_str);
                        if ui.button("Close").clicked() {
                            worker.pop_error();
                        }
                    } else {
                        ui.label("");
                    }
                });
            });
        });

        // The bottom panel is always shown, it allows the user to switch modes.
        TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.columns(3, |columns| {
                columns[0].vertical_centered(|ui| {
                    if ui.button("Assets").clicked() {
                        self.mode = Mode::Assets;
                    }
                });
                columns[1].vertical_centered(|ui| {
                    if ui.button("Send").clicked() {
                        self.mode = Mode::Send;
                    }
                });
                columns[2].vertical_centered(|ui| {
                    if ui.button("Swap").clicked() {
                        self.mode = Mode::Swap;
                    }
                });
            });
        });

        // The central panel the region left after adding TopPanel's and SidePanel's
        // This contains whatever ui elements are needed for the current mode.
        CentralPanel::default().show(ctx, |ui| {
            let token_infos = worker.get_token_info();
            let mut balances = worker.get_balances();

            match self.mode {
                Mode::Assets => {
                    ui.heading("Assets");

                    Grid::new("assets_table").show(ui, |ui| {
                        for token_info in token_infos.iter() {
                            ui.label(token_info.symbol.clone());
                            let value = balances.entry(token_info.token_id).or_default();
                            let value_i64 = i64::try_from(*value).unwrap_or(i64::MAX);
                            let scaled_value = Decimal::new(value_i64, token_info.decimals);
                            ui.label(scaled_value.to_string());
                            ui.end_row();
                        }
                    });
                }
                Mode::Send => {
                    ui.heading("Send");

                    ui.horizontal(|ui| {
                        ui.label("Recipient b58 address: ");
                        ui.text_edit_singleline(&mut self.send_to);
                    });

                    let current_token_info: Option<&TokenInfo> = token_infos
                        .iter()
                        .find(|info| info.token_id == self.send_token_id);

                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label("Token")
                            .selected_text(
                                current_token_info
                                    .map(|info| info.symbol.clone())
                                    .unwrap_or_default(),
                            )
                            .show_ui(ui, |ui| {
                                for info in token_infos.iter() {
                                    ui.selectable_value(
                                        &mut self.send_token_id,
                                        info.token_id,
                                        info.symbol.clone(),
                                    );
                                }
                            });

                        let scaled_value_str = self
                            .send_value
                            .entry(self.send_token_id)
                            .or_insert_with(|| "0".to_string());
                        ui.text_edit_singleline(scaled_value_str);
                    });

                    let scaled_value_str = self
                        .send_value
                        .entry(self.send_token_id)
                        .or_insert_with(|| "0".to_string());

                    // This either the u64 value of the token to send, or a string error to display
                    let okay_to_submit: Result<u64, String> = current_token_info
                        .ok_or("must select a token".to_string())
                        .and_then(|info: &TokenInfo| -> Result<u64, String> {
                            let parsed_value = Decimal::from_str(scaled_value_str)
                                .map_err(|err| err.to_string())?;
                            let scale = Decimal::new(1, info.decimals);
                            let rescaled_value = parsed_value
                                .checked_div(scale)
                                .ok_or("decimal overflow".to_string())?;
                            let u64_value = rescaled_value
                                .round()
                                .to_u64()
                                .ok_or("u64 overflow".to_string())?;

                            let u64_value_with_fee = u64_value
                                .checked_add(info.fee)
                                .ok_or("u64 overflow with fee".to_string())?;
                            if u64_value_with_fee > *balances.entry(self.send_token_id).or_default()
                            {
                                return Err("balance exceeded".to_string());
                            }

                            // Check the send_to field
                            Worker::decode_b58_address(&self.send_to)?;

                            Ok(u64_value)
                        });

                    match okay_to_submit {
                        Ok(u64_value) => {
                            ui.label("");
                            if ui.button("Submit").clicked() {
                                worker.send(u64_value, self.send_token_id, self.send_to.clone());
                            }
                        }
                        Err(err_str) => {
                            ui.label(err_str);
                            ui.add_enabled(false, Button::new("Submit"));
                        }
                    }
                }
                Mode::Swap => {
                    ui.heading("Swap");
                }
            }
        });
    }
}

/*            // The top panel is often a good place for a menu bar:
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("About").clicked() {
                        egui::Window::new("About").show(ctx, |ui| {
                            ui.hyperlink_to(
                                "mobilecoind-buddy",
                                "https://github.com/cbeck88/mobilecoind-buddy",
                            );
                            ui.label("Windows can be moved by dragging them.");
                            ui.label("They are automatically sized based on contents.");
                            ui.label("You can turn on resizing and scrolling if you like.");
                            ui.label("You would normally choose either panels OR windows.");
                        });
                    }
                    if ui.button("Quit").clicked() {
                        _frame.close();
                    }
                });
            });
*/

/*
        egui::SidePanel::left("side_panel").show(ctx, |ui| {
            ui.heading("Side Panel");

            ui.horizontal(|ui| {
                ui.label("Write something: ");
                ui.text_edit_singleline(label);
            });

            ui.add(egui::Slider::new(value, 0.0..=10.0).text("value"));
            if ui.button("Increment").clicked() {
                *value += 1.0;
            }

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label("powered by ");
                    ui.hyperlink_to("egui", "https://github.com/emilk/egui");
                    ui.label(" and ");
                    ui.hyperlink_to(
                        "eframe",
                        "https://github.com/emilk/egui/tree/master/crates/eframe",
                    );
                    ui.label(".");
                });
            });
        });
*/
