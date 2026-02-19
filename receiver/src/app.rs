// ---------------------------------------------------------------------------
// App UI — iced 0.13 application with system tray integration
// ---------------------------------------------------------------------------

use crate::core::{CoreCommand, CoreController, SharedStatus, StatusSnapshot};
use crate::TrayMessage;
use cpal::traits::{DeviceTrait, HostTrait};
use iced::{
    widget::{
        button, checkbox, column, container, horizontal_space, pick_list, qr_code, row,
        scrollable, text, text_input, vertical_space,
    },
    Alignment, Border, Color, Element, Length, Shadow, Subscription, Task, Theme,
};

// ===========================================================================
// Design Tokens — premium dark theme inspired by modern VPN / audio apps
// ===========================================================================

// Backgrounds
const BG_PRIMARY: Color = Color::from_rgb(0.06, 0.07, 0.09);
const BG_ELEVATED: Color = Color::from_rgb(0.10, 0.11, 0.14);
const BG_INPUT: Color = Color::from_rgb(0.14, 0.15, 0.18);
const BG_HOVER: Color = Color::from_rgb(0.16, 0.17, 0.21);

// Borders
const BORDER_SUBTLE: Color = Color::from_rgb(0.20, 0.21, 0.25);

// Text
const TEXT_PRIMARY: Color = Color::from_rgb(0.95, 0.95, 0.97);
const TEXT_SECONDARY: Color = Color::from_rgb(0.55, 0.57, 0.63);
const TEXT_TERTIARY: Color = Color::from_rgb(0.40, 0.42, 0.48);

// Accents
const ACCENT: Color = Color::from_rgb(0.25, 0.56, 0.97);

const SUCCESS: Color = Color::from_rgb(0.20, 0.78, 0.55);
const ERROR: Color = Color::from_rgb(0.95, 0.35, 0.40);
const WARNING: Color = Color::from_rgb(0.95, 0.70, 0.25);

// ===========================================================================
// Launch
// ===========================================================================

pub fn launch_app(
    controller: CoreController,
    shared: SharedStatus,
    tray_rx: std::sync::mpsc::Receiver<TrayMessage>,
) -> iced::Result {
    let output_devices = enumerate_output_devices();
    let selected_output = output_devices.first().cloned();

    // Create window icon (same design as tray, larger for clarity)
    let win_icon_data = crate::icon::create_icon(64);
    let win_icon = iced::window::icon::from_rgba(win_icon_data, 64, 64).ok();

    iced::application("LAN Mic Receiver", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| Theme::Dark)
        .window(iced::window::Settings {
            size: iced::Size::new(400.0, 600.0),
            resizable: false,
            icon: win_icon,
            exit_on_close_request: false,
            ..Default::default()
        })
        .run_with(move || {
            let status = shared.snapshot();
            (
                App {
                    controller,
                    shared,
                    bind_addr: "0.0.0.0:9001".into(),
                    use_stun: false,
                    output_devices,
                    selected_output,
                    active_view: ActiveView::Main,
                    status,
                    pulse_phase: 0.0,
                    qr_data: None,
                    qr_url: None,
                    tray_rx,
                    window_id: None,
                },
                // Fetch the main window ID immediately
                iced::window::get_oldest().map(Message::GotWindowId),
            )
        })
}

// ===========================================================================
// State
// ===========================================================================

/// Which view is currently displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveView {
    Main,
    Settings,
    Logs,
    QrCode,
}

#[derive(Debug, Clone)]
enum Message {
    BindAddressChanged(String),
    UseStunChanged(bool),
    OutputDeviceChanged(String),
    RefreshDevices,
    StartServer,
    StopServer,
    Navigate(ActiveView),
    OpenQr,
    CloseQr,
    Tick,
    Tray(TrayMessage),
    GotWindowId(Option<iced::window::Id>),
    WindowCloseRequested(iced::window::Id),
}

struct App {
    controller: CoreController,
    shared: SharedStatus,

    // Settings
    bind_addr: String,
    use_stun: bool,
    output_devices: Vec<String>,
    selected_output: Option<String>,

    // View state
    active_view: ActiveView,
    status: StatusSnapshot,
    pulse_phase: f32,

    // QR code
    qr_data: Option<qr_code::Data>,
    qr_url: Option<String>,

    // Window & Tray
    window_id: Option<iced::window::Id>,
    tray_rx: std::sync::mpsc::Receiver<TrayMessage>,
}

// ===========================================================================
// Update
// ===========================================================================

impl App {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::BindAddressChanged(addr) => {
                self.bind_addr = addr;
                Task::none()
            }
            Message::UseStunChanged(checked) => {
                self.use_stun = checked;
                Task::none()
            }
            Message::OutputDeviceChanged(device) => {
                self.selected_output = Some(device.clone());
                if self.status.server_running {
                    if let Err(e) = self.controller.send(CoreCommand::ChangeOutputDevice {
                        device_name: Some(device),
                    }) {
                        log::warn!("Failed to send ChangeOutputDevice: {e}");
                    }
                }
                Task::none()
            }
            Message::RefreshDevices => {
                self.output_devices = enumerate_output_devices();
                if self.selected_output.is_none() && !self.output_devices.is_empty() {
                    self.selected_output = self.output_devices.first().cloned();
                }
                Task::none()
            }
            Message::StartServer => {
                if let Err(e) = self.controller.send(CoreCommand::Start {
                    bind_addr: self.bind_addr.clone(),
                    output_device: self.selected_output.clone(),
                    use_stun: self.use_stun,
                }) {
                    log::warn!("Failed to send Start: {e}");
                } else {
                    // Auto-open QR code on start
                    self.active_view = ActiveView::QrCode;
                }
                Task::none()
            }
            Message::StopServer => {
                if let Err(e) = self.controller.send(CoreCommand::Stop) {
                    log::warn!("Failed to send Stop: {e}");
                }
                Task::none()
            }
            Message::Navigate(view) => {
                self.active_view = view;
                Task::none()
            }
            Message::OpenQr => {
                self.active_view = ActiveView::QrCode;
                Task::none()
            }
            Message::CloseQr => {
                self.active_view = ActiveView::Main;
                Task::none()
            }
            Message::Tick => {
                self.status = self.shared.snapshot();
                self.pulse_phase = (self.pulse_phase + 0.08) % (2.0 * std::f32::consts::PI);

                // Regenerate QR code when the URL changes
                let current_url = self.status.ws_url.as_ref().map(|ws| {
                    if ws.starts_with("wss://") {
                         // Convert wss://ip:port/ws -> https://ip:port
                        format!("https://{}", ws.trim_start_matches("wss://").trim_end_matches("/ws"))
                    } else {
                        // Convert ws://ip:port/ws -> http://ip:port
                        format!("http://{}", ws.trim_start_matches("ws://").trim_end_matches("/ws"))
                    }
                });
                let http_url = current_url;
                if http_url != self.qr_url {
                    self.qr_url = http_url.clone();
                    self.qr_data = http_url
                        .and_then(|url| qr_code::Data::new(url).ok());
                }

                // Poll tray messages (non-blocking)
                if let Ok(msg) = self.tray_rx.try_recv() {
                    return self.update(Message::Tray(msg));
                }
                Task::none()
            }
            Message::Tray(msg) => match msg {
                TrayMessage::Show => {
                    if let Some(id) = self.window_id {
                        iced::window::change_mode(id, iced::window::Mode::Windowed)
                    } else {
                        Task::none()
                    }
                }
                TrayMessage::Hide => {
                    if let Some(id) = self.window_id {
                        iced::window::change_mode(id, iced::window::Mode::Hidden)
                    } else {
                        Task::none()
                    }
                }
                TrayMessage::Quit => {
                    // Graceful stop then close
                    let _ = self.controller.send(CoreCommand::Stop);
                    if let Some(id) = self.window_id {
                        iced::window::close(id)
                    } else {
                        std::process::exit(0);
                    }
                }
            },
            Message::GotWindowId(id) => {
                self.window_id = id;
                Task::none()
            }
            Message::WindowCloseRequested(id) => {
                // Hide to tray instead of closing
                self.window_id = Some(id);
                iced::window::change_mode(id, iced::window::Mode::Hidden)
            },
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            // Periodic status polling + tray message check
            iced::time::every(std::time::Duration::from_millis(50)).map(|_| Message::Tick),
            // Intercept window close → hide to tray instead of quitting
            iced::event::listen_with(|event, _status, id| {
                if let iced::Event::Window(iced::window::Event::CloseRequested) = event {
                    Some(Message::WindowCloseRequested(id))
                } else {
                    None
                }
            }),
        ])
    }

    // =======================================================================
    // View — root
    // =======================================================================

    fn view(&self) -> Element<'_, Message> {
        let content = match self.active_view {
            ActiveView::Main => self.main_view(),
            ActiveView::Settings => self.settings_view(),
            ActiveView::Logs => self.logs_view(),
            ActiveView::QrCode => self.qr_view(),
        };

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(BG_PRIMARY.into()),
                ..Default::default()
            })
            .into()
    }

    // =======================================================================
    // Main View
    // =======================================================================

    fn main_view(&self) -> Element<'_, Message> {
        let header = self.header_bar("LAN Mic Receiver", Some(ActiveView::Settings), "Settings");

        let connection = container(self.connection_hero())
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Center);

        let cards = self.info_cards();
        let footer = self.footer_bar();

        column![header, connection, cards, footer]
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(16)
            .padding(24)
            .into()
    }

    // =======================================================================
    // Connection Hero (big button + status)
    // =======================================================================

    fn connection_hero(&self) -> Element<'_, Message> {
        let is_running = self.status.server_running;
        let is_connected = self.status.client_connected;

        let (state_color, state_label, btn_label) = if is_connected {
            (SUCCESS, "Connected", "STOP")
        } else if is_running {
            (WARNING, "Waiting for device…", "STOP")
        } else {
            (TEXT_TERTIARY, "Disconnected", "START")
        };

        // Animated glow intensity
        let pulse = if is_running && !is_connected {
            0.15 + 0.25 * ((self.pulse_phase.sin() + 1.0) / 2.0)
        } else if is_connected {
            0.25
        } else {
            0.0
        };

        // --- Large circular button ---
        let size = 140.0;
        let btn_color = if is_running { ERROR } else { ACCENT };

        let connect_btn = button(
            container(
                text(btn_label)
                    .size(14)
                    .style(move |_| text::Style {
                        color: Some(btn_color),
                    }),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
        )
        .on_press(if is_running {
            Message::StopServer
        } else {
            Message::StartServer
        })
        .style(move |_, _| button::Style {
            background: Some(BG_ELEVATED.into()),
            text_color: btn_color,
            border: Border {
                color: btn_color,
                width: 2.0,
                radius: (size / 2.0).into(),
            },
            shadow: Shadow {
                color: state_color.scale_alpha(pulse),
                offset: iced::Vector::ZERO,
                blur_radius: 30.0,
            },
        })
        .width(Length::Fixed(size))
        .height(Length::Fixed(size));

        // Outer glow ring
        let glow_ring = container(
            container(connect_btn)
                .style(move |_| container::Style {
                    background: Some(state_color.scale_alpha(pulse * 0.4).into()),
                    border: Border {
                        color: state_color.scale_alpha(pulse),
                        width: 1.0,
                        radius: ((size + 16.0) / 2.0).into(),
                    },
                    ..Default::default()
                })
                .padding(8),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center);

        // Status dot + label
        let dot = container(text(""))
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0))
            .style(move |_| container::Style {
                background: Some(state_color.into()),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });

        let status_row = container(
            row![
                dot,
                text(state_label).size(13).style(move |_| text::Style {
                    color: Some(state_color),
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center);

        // Subtitle
        let subtitle = if is_connected {
            self.status
                .client_addr
                .as_deref()
                .map(|a| format!("Device connected from {a}"))
                .unwrap_or_else(|| "Audio streaming active".into())
        } else if is_running {
            self.status
                .ws_url
                .as_deref()
                .map(|u| format!("Listening on {u}"))
                .unwrap_or_else(|| "Starting…".into())
        } else {
            "Ready to receive audio from your device".into()
        };

        let subtitle_text = container(
            text(subtitle)
                .size(12)
                .style(|_| text::Style {
                    color: Some(TEXT_SECONDARY),
                })
                .align_x(iced::alignment::Horizontal::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center);

        column![
            glow_ring,
            vertical_space().height(20),
            status_row,
            subtitle_text,
        ]
        .align_x(Alignment::Center)
        .spacing(8)
        .into()
    }

    // =======================================================================
    // QR Code Card
    // =======================================================================


    // =======================================================================
    // QR Code View (Modal-like)
    // =======================================================================

    fn qr_view(&self) -> Element<'_, Message> {
        let content = match (&self.qr_data, &self.qr_url) {
            (Some(data), Some(url)) => {
                let qr = container(
                    qr_code(data)
                        .cell_size(6)
                        .style(|_| qr_code::Style {
                            background: Color::WHITE,
                            cell: Color::from_rgb(0.06, 0.07, 0.09),
                        }),
                )
                .padding(20)
                .style(|_| container::Style {
                    background: Some(Color::WHITE.into()),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 16.0.into(),
                    },
                    ..Default::default()
                });

                let url_label = text(url.as_str())
                    .size(16)
                    .font(iced::Font::MONOSPACE)
                    .style(|_| text::Style {
                        color: Some(TEXT_SECONDARY),
                    });

                let instructions = text("Scan with your phone to open the web sender")
                    .size(14)
                    .align_x(iced::alignment::Horizontal::Center)
                    .style(|_| text::Style {
                        color: Some(TEXT_TERTIARY),
                    });

                let close_btn = button(text("Close").size(14))
                    .on_press(Message::CloseQr)
                    .padding([10, 24])
                    .style(ghost_button_style);

                column![qr, vertical_space().height(20), url_label, instructions, vertical_space().height(20), close_btn]
                    .spacing(12)
                    .align_x(Alignment::Center)
            }
            _ => column![
                text("Starting server…").size(14),
                button("Close").on_press(Message::CloseQr)
            ]
            .spacing(20)
            .align_x(Alignment::Center),
        };

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into()
    }

    // =======================================================================
    // Info Cards (output device + packet stats)
    // =======================================================================

    fn info_cards(&self) -> Element<'_, Message> {
        let device_name = self
            .selected_output
            .as_deref()
            .unwrap_or("No device selected");
        let device_label = truncate_str(device_name, 40);

        let audio_card = self.card(
            "OUTPUT DEVICE",
            text(device_label)
                .size(13)
                .style(|_| text::Style {
                    color: Some(TEXT_PRIMARY),
                })
                .into(),
        );

        let packets = self.status.audio_packets;
        let stats_card = self.card(
            "PACKETS RECEIVED",
            text(packets.to_string())
                .size(20)
                .font(iced::Font::MONOSPACE)
                .style(|_| text::Style {
                    color: Some(ACCENT),
                })
                .into(),
        );

        column![audio_card, stats_card].spacing(12).into()
    }

    // =======================================================================
    // Settings View
    // =======================================================================

    fn settings_view(&self) -> Element<'_, Message> {
        let header = self.header_bar("Settings", Some(ActiveView::Main), "Back");

        // Server configuration
        let server_card = container(
            column![
                section_title("Server Configuration"),
                vertical_space().height(16),
                label("Bind Address"),
                vertical_space().height(6),
                text_input("0.0.0.0:9001", &self.bind_addr)
                    .on_input(Message::BindAddressChanged)
                    .style(text_input_style)
                    .padding(12),
                vertical_space().height(16),
                checkbox("Use STUN server for NAT traversal", self.use_stun)
                    .on_toggle(Message::UseStunChanged)
                    .style(checkbox_style),
            ]
            .spacing(4),
        )
        .style(card_style)
        .padding(20)
        .width(Length::Fill);

        // Audio output
        let audio_card = container(
            column![
                row![
                    section_title("Audio Output"),
                    horizontal_space(),
                    button(text("Refresh").size(12).style(|_| text::Style {
                        color: Some(ACCENT),
                    }))
                    .on_press(Message::RefreshDevices)
                    .style(ghost_button_style)
                    .padding([4, 8]),
                ]
                .align_y(Alignment::Center),
                vertical_space().height(16),
                pick_list(
                    self.output_devices.clone(),
                    self.selected_output.clone(),
                    Message::OutputDeviceChanged,
                )
                .style(pick_list_style)
                .placeholder("Select audio device…")
                .width(Length::Fill),
            ]
            .spacing(4),
        )
        .style(card_style)
        .padding(20)
        .width(Length::Fill);

        // Tip
        let tip_card = container(
            column![
                text("Tip").size(12).style(|_| text::Style {
                    color: Some(WARNING),
                }),
                vertical_space().height(6),
                text("Install VB-Cable and select 'CABLE Input' to route audio to other apps like Discord or OBS.")
                    .size(12)
                    .style(|_| text::Style {
                        color: Some(TEXT_SECONDARY),
                    }),
            ]
            .spacing(4),
        )
        .style(card_style)
        .padding(20)
        .width(Length::Fill);

        let content = column![header, server_card, audio_card, tip_card].spacing(12);

        scrollable(content.padding(24))
            .height(Length::Fill)
            .into()
    }

    // =======================================================================
    // Logs View
    // =======================================================================

    fn logs_view(&self) -> Element<'_, Message> {
        let header = self.header_bar("System Logs", Some(ActiveView::Main), "Back");

        let log_text = if self.status.log_lines.is_empty() {
            "No logs yet…".to_string()
        } else {
            // Show up to last 100 lines
            let start = self.status.log_lines.len().saturating_sub(100);
            self.status.log_lines[start..].join("\n")
        };

        let log_container = container(
            scrollable(
                container(
                    text(log_text)
                        .font(iced::Font::MONOSPACE)
                        .size(11)
                        .style(|_| text::Style {
                            color: Some(TEXT_SECONDARY),
                        }),
                )
                .padding(16)
                .width(Length::Fill),
            )
            .height(Length::Fill),
        )
        .style(card_style)
        .width(Length::Fill)
        .height(Length::Fill);

        column![header, vertical_space().height(12), log_container]
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(24)
            .into()
    }

    // =======================================================================
    // Reusable Components
    // =======================================================================

    /// Header bar with a title and optional navigation button.
    fn header_bar<'a>(
        &self,
        title: &'a str,
        nav_target: Option<ActiveView>,
        nav_label: &'a str,
    ) -> Element<'a, Message> {
        let title_text = text(title).size(17).style(|_| text::Style {
            color: Some(TEXT_PRIMARY),
        });

        match nav_target {
            Some(target) if nav_label == "Back" => {
                // Back button on the left
                row![
                    button(
                        text("< Back").size(13).style(|_| text::Style {
                            color: Some(TEXT_SECONDARY),
                        }),
                    )
                    .on_press(Message::Navigate(target))
                    .style(ghost_button_style)
                    .padding([6, 6]),
                    horizontal_space().width(8),
                    title_text,
                ]
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .into()
            }
            Some(target) => {
                // Action button on the right
                let settings_btn = button(text(nav_label).size(16).style(|_| text::Style {
                    color: Some(TEXT_SECONDARY),
                }))
                .on_press(Message::Navigate(target))
                .style(ghost_button_style)
                .padding([6, 10]);

                let mut row_content = row![title_text, horizontal_space()];

                // Add QR button if we are on the main screen (indicated by "Settings" label)
                if nav_label == "Settings" {
                    row_content = row_content.push(
                        button(text("QR").size(14).style(|_| text::Style {
                            color: Some(TEXT_SECONDARY),
                        }))
                        .on_press(Message::OpenQr)
                        .style(ghost_button_style)
                        .padding([6, 10]),
                    );
                }

                row_content.push(settings_btn)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .into()
            }
            None => {
                row![title_text]
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .into()
            }
        }
    }

    /// A styled card with a header label and arbitrary content.
    fn card<'a>(&self, header: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
        container(
            column![
                text(header).size(10).style(|_| text::Style {
                    color: Some(TEXT_TERTIARY),
                }),
                vertical_space().height(6),
                content,
            ]
            .spacing(2),
        )
        .style(card_style)
        .padding(16)
        .width(Length::Fill)
        .into()
    }

    /// Footer bar with version and log button.
    fn footer_bar(&self) -> Element<'_, Message> {
        row![
            text("LAN Mic Receiver v0.1")
                .size(11)
                .style(|_| text::Style {
                    color: Some(TEXT_TERTIARY),
                }),
            horizontal_space(),
            button(text("View Logs").size(11).style(|_| text::Style {
                color: Some(TEXT_SECONDARY),
            }))
            .on_press(Message::Navigate(ActiveView::Logs))
            .style(ghost_button_style)
            .padding([4, 8]),
        ]
        .align_y(Alignment::Center)
        .into()
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn enumerate_output_devices() -> Vec<String> {
    match cpal::default_host().output_devices() {
        Ok(devs) => {
            let mut devices: Vec<String> = devs.filter_map(|d| d.name().ok()).collect();
            devices.sort();
            devices
        }
        Err(_) => vec!["(could not enumerate devices)".into()],
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max.saturating_sub(1)])
    } else {
        s.to_string()
    }
}

fn section_title(label: &str) -> Element<'_, Message> {
    text(label)
        .size(14)
        .style(|_| text::Style {
            color: Some(TEXT_PRIMARY),
        })
        .into()
}

fn label(label: &str) -> Element<'_, Message> {
    text(label)
        .size(12)
        .style(|_| text::Style {
            color: Some(TEXT_SECONDARY),
        })
        .into()
}

// ===========================================================================
// Styles
// ===========================================================================

fn card_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(BG_ELEVATED.into()),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

fn ghost_button_style(_: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => BG_HOVER,
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(bg.into()),
        text_color: TEXT_SECONDARY,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

fn text_input_style(_: &Theme, status: text_input::Status) -> text_input::Style {
    let (border_color, border_width) = match status {
        text_input::Status::Focused => (ACCENT, 1.5),
        _ => (BORDER_SUBTLE, 1.0),
    };
    text_input::Style {
        background: BG_INPUT.into(),
        border: Border {
            color: border_color,
            width: border_width,
            radius: 8.0.into(),
        },
        icon: TEXT_SECONDARY,
        placeholder: TEXT_TERTIARY,
        value: TEXT_PRIMARY,
        selection: ACCENT.scale_alpha(0.3),
    }
}

fn checkbox_style(_: &Theme, status: checkbox::Status) -> checkbox::Style {
    let is_checked = matches!(
        status,
        checkbox::Status::Active { is_checked: true }
            | checkbox::Status::Hovered { is_checked: true }
    );

    checkbox::Style {
        background: if is_checked {
            ACCENT.scale_alpha(0.2).into()
        } else {
            BG_INPUT.into()
        },
        icon_color: if is_checked {
            TEXT_PRIMARY
        } else {
            Color::TRANSPARENT
        },
        border: Border {
            color: if is_checked { ACCENT } else { BORDER_SUBTLE },
            width: 1.5,
            radius: 4.0.into(),
        },
        text_color: Some(TEXT_SECONDARY),
    }
}

fn pick_list_style(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let border_color = match status {
        pick_list::Status::Hovered | pick_list::Status::Opened => ACCENT,
        _ => BORDER_SUBTLE,
    };
    pick_list::Style {
        background: BG_INPUT.into(),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: 8.0.into(),
        },
        placeholder_color: TEXT_TERTIARY,
        handle_color: TEXT_SECONDARY,
        text_color: TEXT_PRIMARY,
    }
}
