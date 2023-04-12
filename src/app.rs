use crate::{Amount, Config, QuoteSelection, TokenId, TokenInfo, Worker};
use egui::{
    Align, Button, CentralPanel, Color32, ComboBox, Grid, Layout, RichText, ScrollArea,
    TopBottomPanel,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{event, Level};

/// The three panels the app can show
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
enum Mode {
    #[default]
    Assets,
    Send,
    Swap,
    OfferSwap,
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
    /// Which token value we most recently selected to swap from (per swap_from_token_id)
    swap_from_value: HashMap<TokenId, String>,
    /// Which token we most recently selected to swap to
    swap_to_token_id: TokenId,
    /// Which token value we most recently selected to swap for (per swap_to_token_id)
    swap_to_value: HashMap<TokenId, String>,
    /// The base token id in the offer_swap pane
    base_token_id: TokenId,
    /// The counter token id in the offer_swap pane
    counter_token_id: TokenId,
    /// The price in the offer_swap pane
    offer_price: String,
    /// The volume in the offer_swap pane
    offer_volume: String,
    /// The worker is doing balance checking with mobilecoind in the background,
    /// and fetching a quotebook from deqs if available.
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
            swap_from_value: Default::default(),
            swap_to_token_id: TokenId::from(1),
            swap_to_value: Default::default(),
            base_token_id: TokenId::from(0),
            counter_token_id: TokenId::from(1),
            offer_price: Default::default(),
            offer_volume: Default::default(),
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

    /// Helper which renders a drop-down menu for selecting a token-id, followed by a text edit line for a value.
    ///
    /// Arguments:
    /// * ui which we are rendering into
    /// * context string, which generates egui ids. Should be unique.
    /// * token_infos, obtained from worker.get_token_infos
    /// * token_id, mutable reference to state this widget is selecting
    /// * values, mutable reference to the value strings this widget is selecting. These are parsed as scaled decimal values.
    fn amount_selector(
        ui: &mut egui::Ui,
        context: &str,
        token_infos: &[TokenInfo],
        token_id: &mut TokenId,
        values: &mut HashMap<TokenId, String>,
    ) {
        let current_token_info: Option<&TokenInfo> =
            token_infos.iter().find(|info| info.token_id == *token_id);

        ui.horizontal(|ui| {
            ui.label(context);
            ComboBox::from_id_source(context)
                .selected_text(
                    current_token_info
                        .map(|info| info.symbol.clone())
                        .unwrap_or_default(),
                )
                .show_ui(ui, |ui| {
                    for info in token_infos.iter() {
                        ui.selectable_value(token_id, info.token_id, info.symbol.clone());
                    }
                });

            let scaled_value_str = values.entry(*token_id).or_insert_with(|| "0".to_string());
            ui.text_edit_singleline(scaled_value_str);
        });
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

        // Makes the font appear large enough to read
        ctx.set_pixels_per_point(4.0);
        // Make the app redraw itself even without movement
        ctx.request_repaint_after(Duration::from_millis(100));

        // The top panel is always shown no matter what mode we are in,
        // it shows the public address and sync %
        TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                // Add a display of the public address, and a copy button
                let public_address = worker.get_b58_address();
                if ui
                    .button(format!(
                        "Public address: {}...{} 📋",
                        &public_address[..8],
                        &public_address[public_address.len() - 8..]
                    ))
                    .clicked()
                {
                    ui.output_mut(|o| o.copied_text = public_address);
                }

                // Add a display of the sync %
                let (synced_blocks, total_blocks) = worker.get_sync_progress();
                let fraction = synced_blocks as f64 / total_blocks as f64;
                let sync_percent = format!("{:.1}", fraction * 100f64);
                ui.label(format!(
                    "Ledger sync: {sync_percent}% ({synced_blocks} / {total_blocks})"
                ));

                // Add a warning if we have a debug build
                egui::warn_if_debug_build(ui);

                // Check if the worker has reported any error, if so, show it
                ui.horizontal(|ui| {
                    if let Some(err_str) = worker.top_error() {
                        if ui.button("⊗").clicked() {
                            worker.pop_error();
                        }
                        ui.label(RichText::new(err_str).color(Color32::from_rgb(255, 0, 0)));
                    } else {
                        ui.label("");
                    }
                });
            });
        });

        // The bottom panel is always shown, it allows the user to switch modes.
        TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.columns(4, |columns| {
                columns[0].vertical_centered(|ui| {
                    if ui.button("Assets").clicked() {
                        self.mode = Mode::Assets;
                        worker.stop_quotes();
                    }
                });
                columns[1].vertical_centered(|ui| {
                    if ui.button("Send").clicked() {
                        self.mode = Mode::Send;
                        worker.stop_quotes();
                    }
                });
                columns[2].vertical_centered(|ui| {
                    if ui.button("Swap").clicked() {
                        self.mode = Mode::Swap;
                        worker.get_quotes_for_token_ids(
                            self.swap_to_token_id,
                            self.swap_from_token_id,
                        );
                    }
                });
                columns[3].vertical_centered(|ui| {
                    if ui.button("Offer Swap").clicked() {
                        self.mode = Mode::OfferSwap;
                        worker.get_quotes_for_token_ids(
                            self.swap_to_token_id,
                            self.swap_from_token_id,
                        );
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

                    Self::amount_selector(
                        ui,
                        "Amount",
                        &token_infos,
                        &mut self.send_token_id,
                        &mut self.send_value,
                    );

                    let current_token_info: Option<&TokenInfo> = token_infos
                        .iter()
                        .find(|info| info.token_id == self.send_token_id);

                    let scaled_value_str = self
                        .send_value
                        .entry(self.send_token_id)
                        .or_insert_with(|| "0".to_string());

                    match current_token_info.as_ref() {
                        Some(info) => {
                            let scale = Decimal::new(1, info.decimals);
                            if let Some(balance) =
                                Decimal::from(*balances.entry(self.send_token_id).or_default())
                                    .checked_mul(scale)
                            {
                                ui.label(format!("balance: {}", balance));
                            } else {
                                ui.label("balance: (overflow)");
                            }
                            if let Some(fee) = Decimal::from(info.fee).checked_mul(scale) {
                                ui.label(format!("fee: {}", fee));
                            } else {
                                ui.label("fee: (overflow)");
                            }
                        }
                        None => {
                            ui.label("balance:");
                            ui.label("fee:");
                        }
                    }

                    // This either the u64 value of the token to send, or a string error to display
                    let okay_to_submit: Result<u64, String> = current_token_info
                        .ok_or("select a token".to_string())
                        .and_then(|info: &TokenInfo| -> Result<u64, String> {
                            let u64_value = info.try_scaled_to_u64(scaled_value_str)?;

                            let u64_value_with_fee = u64_value
                                .checked_add(info.fee)
                                .ok_or("u64 overflow with fee".to_string())?;
                            if u64_value_with_fee > *balances.entry(self.send_token_id).or_default()
                            {
                                return Err("insufficient funds".to_string());
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

                    if !worker.has_deqs() {
                        ui.label("No deqs uri was configured, swap is not available.");
                        return;
                    }

                    Self::amount_selector(
                        ui,
                        "Swap from",
                        &token_infos,
                        &mut self.swap_from_token_id,
                        &mut self.swap_from_value,
                    );
                    ui.label("↓");
                    Self::amount_selector(
                        ui,
                        "Swap to",
                        &token_infos,
                        &mut self.swap_to_token_id,
                        &mut self.swap_to_value,
                    );

                    worker.get_quotes_for_token_ids(self.swap_to_token_id, self.swap_from_token_id);

                    let quote_book =
                        worker.get_quote_book(self.swap_to_token_id, self.swap_from_token_id);

                    let swap_from_token_info: Option<&TokenInfo> = token_infos
                        .iter()
                        .find(|info| info.token_id == self.swap_from_token_id);

                    let swap_to_token_info: Option<&TokenInfo> = token_infos
                        .iter()
                        .find(|info| info.token_id == self.swap_to_token_id);

                    // Returns an SCI we selected to swap against, and the partial fill value to fill it to, or an error message
                    let okay_to_submit: Result<QuoteSelection, String> = swap_from_token_info
                        .zip(swap_to_token_info)
                        .ok_or("".to_string())
                        .and_then(|(from_info, to_info)| -> Result<QuoteSelection, String> {
                            if self.swap_from_token_id == self.swap_to_token_id {
                                return Err("".to_string());
                            }

                            let to_u64_value = to_info.try_scaled_to_u64(
                                self.swap_to_value
                                    .entry(self.swap_to_token_id)
                                    .or_insert_with(|| "0".to_string()),
                            )?;

                            let to_amount = Amount::new(to_u64_value, self.swap_to_token_id);

                            // TODO: If the user is modifying the swap_from_value field, it would be nice to do
                            // quote selection based on that, and update the swap_to_value field. Uniswap works this way.
                            // At this revision we only pay attention to the swap_to_value field, and always update swap_from_value
                            // based on that.
                            let qs = QuoteSelection::new(
                                &quote_book,
                                self.swap_from_token_id,
                                from_info,
                                to_amount,
                            )?;

                            // Check if we have sufficient funds to do this
                            let from_token_balance =
                                balances.get(&self.swap_from_token_id).cloned().unwrap_or(0);
                            let from_token_fee = from_info.fee;
                            if from_token_balance < qs.from_u64_value + from_token_fee {
                                return Err("insufficient funds".to_string());
                            }
                            Ok(qs)
                        });

                    match okay_to_submit {
                        Ok(qs) => {
                            *self
                                .swap_from_value
                                .entry(self.swap_from_token_id)
                                .or_default() = qs.from_value_decimal.to_string();
                            ui.label("");
                            if ui.button("Submit").clicked() {
                                // We pay the fee in the from_token_id
                                let fee_token_id = self.swap_from_token_id;
                                worker.perform_swap(
                                    qs.sci,
                                    qs.partial_fill_value,
                                    self.swap_from_token_id,
                                    fee_token_id,
                                );
                            }
                        }
                        Err(err_str) => {
                            ui.label(err_str);
                            ui.add_enabled(false, Button::new("Submit"));
                        }
                    }
                }
                Mode::OfferSwap => {
                    ui.heading("Offer Swap");

                    if !worker.has_deqs() {
                        ui.label("No deqs uri was configured, swap is not available.");
                        return;
                    }

                    let base_token_info: Option<&TokenInfo> = token_infos
                        .iter()
                        .find(|info| info.token_id == self.base_token_id);

                    let counter_token_info: Option<&TokenInfo> = token_infos
                        .iter()
                        .find(|info| info.token_id == self.counter_token_id);

                    // Show the asset pair as two side-by-side drop-down menus
                    ui.horizontal(|ui| {
                        ComboBox::from_id_source("base_token_id")
                            .selected_text(
                                base_token_info
                                    .map(|info| info.symbol.clone())
                                    .unwrap_or_default(),
                            )
                            .show_ui(ui, |ui| {
                                for info in token_infos.iter() {
                                    ui.selectable_value(
                                        &mut self.base_token_id,
                                        info.token_id,
                                        info.symbol.clone(),
                                    );
                                }
                            });
                        ui.label("/");
                        ComboBox::from_id_source("counter_token_id")
                            .selected_text(
                                counter_token_info
                                    .map(|info| info.symbol.clone())
                                    .unwrap_or_default(),
                            )
                            .show_ui(ui, |ui| {
                                for info in token_infos.iter() {
                                    ui.selectable_value(
                                        &mut self.counter_token_id,
                                        info.token_id,
                                        info.symbol.clone(),
                                    );
                                }
                            });
                    });

                    worker.get_quotes_for_token_ids(self.base_token_id, self.counter_token_id);

                    // In these states, we can't proceed, don't render any more ui.
                    if self.base_token_id == self.counter_token_id {
                        return;
                    }

                    let base_token_info = match base_token_info {
                        Some(base_token_info) => base_token_info,
                        None => {
                            return;
                        }
                    };

                    let counter_token_info = match counter_token_info {
                        Some(counter_token_info) => counter_token_info,
                        None => {
                            return;
                        }
                    };

                    // User-specified price for base-token in terms of counter token
                    ui.horizontal(|ui| {
                        ui.label(format!("Price ({})", counter_token_info.symbol.clone()));
                        ui.text_edit_singleline(&mut self.offer_price);
                    });
                    ui.horizontal(|ui| {
                        ui.label(format!("Volume ({})", base_token_info.symbol.clone()));
                        ui.text_edit_singleline(&mut self.offer_volume);
                    });

                    let base_volume =
                        Decimal::from_str(&self.offer_volume).map_err(|err| err.to_string());
                    let price = Decimal::from_str(&self.offer_price).map_err(|err| err.to_string());
                    let counter_volume = base_volume.clone().and_then(|base_volume_decimal| {
                        price.and_then(|price_decimal| {
                            base_volume_decimal
                                .checked_mul(price_decimal)
                                .ok_or_else(|| "decimal overflow".to_owned())
                        })
                    });
                    let base_u64_value = base_volume
                        .and_then(|base_vol| base_token_info.try_decimal_to_u64(base_vol));
                    let counter_u64_value = counter_volume
                        .and_then(|counter_vol| counter_token_info.try_decimal_to_u64(counter_vol));

                    // Computes the hint text for the buy button. The result is Ok if we can buy,
                    // and Err if we cannot buy for some reason.
                    let buy_is_possible: Result<String, String> =
                        counter_u64_value.clone().and_then(|counter_u64_value| {
                            base_u64_value.clone().and_then(|base_u64_value| {
                                if *balances.entry(self.counter_token_id).or_default()
                                    >= counter_u64_value
                                {
                                    // FIXME: check for i64 overflow
                                    Ok(format!(
                                        "Offer to trade {} {}\n for {} {}",
                                        Decimal::new(
                                            counter_u64_value as i64,
                                            counter_token_info.decimals
                                        ),
                                        counter_token_info.symbol,
                                        Decimal::new(
                                            base_u64_value as i64,
                                            base_token_info.decimals
                                        ),
                                        base_token_info.symbol
                                    ))
                                } else {
                                    Err(format!("Insufficient {}", counter_token_info.symbol))
                                }
                            })
                        });
                    let buy_hint_text = match buy_is_possible.as_ref() {
                        Ok(text) => text,
                        Err(text) => text,
                    };

                    // Computes the hint text for the sell button. The result is Ok if we can sell,
                    // and Err if we cannot sell for some reason.
                    let sell_is_possible: Result<String, String> =
                        base_u64_value.clone().and_then(|base_u64_value| {
                            counter_u64_value.clone().and_then(|counter_u64_value| {
                                if *balances.entry(self.base_token_id).or_default()
                                    >= base_u64_value
                                {
                                    // FIXME: check for i64 overflow
                                    Ok(format!(
                                        "Offer to trade {} {}\n for {} {}",
                                        Decimal::new(
                                            base_u64_value as i64,
                                            base_token_info.decimals
                                        ),
                                        base_token_info.symbol,
                                        Decimal::new(
                                            counter_u64_value as i64,
                                            counter_token_info.decimals
                                        ),
                                        counter_token_info.symbol
                                    ))
                                } else {
                                    Err(format!("Insufficient {}", base_token_info.symbol))
                                }
                            })
                        });
                    let sell_hint_text = match sell_is_possible.as_ref() {
                        Ok(text) => text,
                        Err(text) => text,
                    };

                    // Add buy and sell buttons
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(buy_is_possible.is_ok(), Button::new("Buy"))
                            .on_hover_text(buy_hint_text)
                            .on_disabled_hover_text(buy_hint_text)
                            .clicked()
                        {
                            let from_amount = Amount::new(
                                counter_u64_value.clone().unwrap(),
                                self.counter_token_id,
                            );
                            let to_amount =
                                Amount::new(base_u64_value.clone().unwrap(), self.base_token_id);
                            worker.offer_swap(from_amount, to_amount);
                        }
                        if ui
                            .add_enabled(sell_is_possible.is_ok(), Button::new("Sell"))
                            .on_hover_text(sell_hint_text)
                            .on_disabled_hover_text(sell_hint_text)
                            .clicked()
                        {
                            let from_amount =
                                Amount::new(base_u64_value.unwrap(), self.base_token_id);
                            let to_amount =
                                Amount::new(counter_u64_value.unwrap(), self.counter_token_id);
                            worker.offer_swap(from_amount, to_amount);
                        }
                    });

                    ui.separator();

                    // Show the quote book

                    let books = [
                        worker.get_quote_book(self.swap_to_token_id, self.swap_from_token_id),
                        worker.get_quote_book(self.swap_from_token_id, self.swap_to_token_id),
                    ];
                    let headings = ["Bid", "Ask"];

                    ScrollArea::vertical().show(ui, |ui| {
                        ui.columns(2, |columns| {
                            for idx in 0..2 {
                                let ui = &mut columns[idx];

                                ui.heading(headings[idx]);

                                Grid::new(format!("{}_table", headings[idx])).show(ui, |ui| {
                                    ui.label("Price              ");
                                    ui.label("Volume             ");
                                    ui.end_row();

                                    for validated_quote in books.get(idx).unwrap() {
                                        match validated_quote.get_quote_info(
                                            self.base_token_id,
                                            self.counter_token_id,
                                            &token_infos,
                                        ) {
                                            Ok(info) => {
                                                ui.label(info.price.to_string());
                                                ui.label(info.volume.to_string());
                                                ui.end_row();
                                            }
                                            Err(err) => {
                                                event!(Level::ERROR, "get quote info: {}", err);
                                            }
                                        }
                                    }
                                });
                            }
                        });
                    });
                }
            }
        });
    }
}
