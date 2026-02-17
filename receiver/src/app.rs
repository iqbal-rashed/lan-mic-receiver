use crate::core::{CoreCommand, CoreController, SharedStatus, StatusSnapshot};
use cpal::traits::{DeviceTrait, HostTrait};
use iced::{
    widget::{
        button, checkbox, column, container, horizontal_space, pick_list, row, scrollable, text,
        text_input, Space,
    },
    Alignment, Border, Color, Element, Length, Subscription, Task, Theme,
};

pub fn launch_app(controller: CoreController, shared: SharedStatus) -> iced::Result {
    let output_devices = enumerate_output_devices();
    let selected_output = output_devices.first().cloned();

    iced::application("LAN Mic Receiver", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| Theme::Dark)
        .window(iced::window::Settings {
            size: iced::Size::new(520.0, 420.0),
            resizable: true,
            ..Default::default()
        })
        .run_with(move || {
            (
                App {
                    controller: controller.clone(),
                    shared: shared.clone(),
                    bind_addr: "0.0.0.0:9001".to_string(),
                    use_stun: false,
                    output_devices,
                    selected_output,
                    show_logs: false,
                    status: shared.snapshot(),
                },
                Task::none(),
            )
        })
}

#[derive(Debug, Clone)]
enum Message {
    BindAddressChanged(String),
    UseStunChanged(bool),
    OutputDeviceChanged(String),
    RefreshDevices,
    StartServer,
    StopServer,
    ToggleLogs,
    Tick,
}

struct App {
    controller: CoreController,
    shared: SharedStatus,
    bind_addr: String,
    use_stun: bool,
    output_devices: Vec<String>,
    selected_output: Option<String>,
    show_logs: bool,
    status: StatusSnapshot,
}

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
                // Live-switch if server is already running
                if self.status.server_running {
                    let _ = self.controller.send(CoreCommand::ChangeOutputDevice {
                        device_name: Some(device),
                    });
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
                let _ = self.controller.send(CoreCommand::Start {
                    bind_addr: self.bind_addr.clone(),
                    output_device: self.selected_output.clone(),
                    use_stun: self.use_stun,
                });
                Task::none()
            }
            Message::StopServer => {
                let _ = self.controller.send(CoreCommand::Stop);
                Task::none()
            }
            Message::ToggleLogs => {
                self.show_logs = !self.show_logs;
                Task::none()
            }
            Message::Tick => {
                self.status = self.shared.snapshot();
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(std::time::Duration::from_millis(100)).map(|_| Message::Tick)
    }

    fn view(&self) -> Element<'_, Message> {
        let server_section = self.server_control_view();
        let audio_section = self.audio_output_view();
        let connection_section = self.connection_status_view();

        let main_content = column![
            server_section,
            audio_section,
            connection_section,
            row![
                horizontal_space(),
                button(if self.show_logs {
                    "Hide Logs"
                } else {
                    "Show Logs"
                })
                .on_press(Message::ToggleLogs)
                .style(button::secondary),
            ]
            .spacing(10),
        ]
        .spacing(10)
        .padding(12)
        .width(Length::Fill);

        if self.show_logs {
            let log_section = self.logs_view();

            row![
                container(main_content).width(Length::FillPortion(1)),
                container(log_section).width(Length::FillPortion(1)),
            ]
            .spacing(12)
            .padding(8)
            .into()
        } else {
            container(main_content)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    fn server_control_view(&self) -> Element<'_, Message> {
        let can_start = !self.status.server_running;
        let can_stop = self.status.server_running;

        let status_row = if self.status.server_running {
            let mut row_content = row![text("●"), text("Running"),]
                .spacing(4)
                .align_y(Alignment::Center);

            if let Some(url) = &self.status.ws_url {
                row_content = row_content.push(Space::with_width(Length::Fixed(12.0)));
                row_content = row_content.push(text(format!("@ {}", url)).size(11));
            }

            row_content
        } else {
            row![text("○"), text("Stopped"),]
                .spacing(4)
                .align_y(Alignment::Center)
        };

        let content = column![
            row![text("Server").size(13), horizontal_space(), status_row,]
                .align_y(Alignment::Center),
            text_input("Bind Address", &self.bind_addr)
                .on_input(Message::BindAddressChanged)
                .padding(8)
                .width(Length::Fill),
            checkbox("Use STUN (NAT traversal)", self.use_stun).on_toggle(Message::UseStunChanged),
            row![
                button("Start")
                    .on_press_maybe(if can_start {
                        Some(Message::StartServer)
                    } else {
                        None
                    })
                    .style(button::success),
                button("Stop")
                    .on_press_maybe(if can_stop {
                        Some(Message::StopServer)
                    } else {
                        None
                    })
                    .style(button::danger),
            ]
            .spacing(8),
        ]
        .spacing(8);

        container(content).padding(10).style(card_style).into()
    }

    fn audio_output_view(&self) -> Element<'_, Message> {
        let content = column![
            row![
                text("Audio Output").size(13),
                horizontal_space(),
                button("Refresh")
                    .on_press(Message::RefreshDevices)
                    .padding([4, 10])
                    .style(button::secondary),
            ]
            .align_y(Alignment::Center),
            pick_list(
                self.output_devices.clone(),
                self.selected_output.clone(),
                Message::OutputDeviceChanged,
            )
            .placeholder("Select device...")
            .width(Length::Fill),
            text("Tip: Select 'CABLE Input' for VB-Cable")
                .size(10)
                .style(text::secondary),
        ]
        .spacing(6);

        container(content).padding(10).style(card_style).into()
    }

    fn connection_status_view(&self) -> Element<'_, Message> {
        let client_status = if self.status.client_connected {
            text("Connected")
        } else {
            text("Not connected").style(text::secondary)
        };

        let pc_state = self.status.pc_state.as_deref().unwrap_or("-");

        let content = column![
            text("Connection").size(13),
            row![text("Client:").width(Length::Fixed(90.0)), client_status,]
                .align_y(Alignment::Center),
            row![text("PeerConn:").width(Length::Fixed(90.0)), text(pc_state),]
                .align_y(Alignment::Center),
            row![
                text("Packets:").width(Length::Fixed(90.0)),
                text(format!("{}", self.status.audio_packets)).font(iced::Font::MONOSPACE),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(6);

        container(content).padding(10).style(card_style).into()
    }

    fn logs_view(&self) -> Element<'_, Message> {
        let log_text: String = self
            .status
            .log_lines
            .iter()
            .rev()
            .take(100)
            .rev()
            .map(|line| format!("> {}", line))
            .collect::<Vec<_>>()
            .join("\n");

        let content = column![
            row![
                text("Logs").size(13),
                horizontal_space(),
                button("Hide")
                    .on_press(Message::ToggleLogs)
                    .style(button::secondary)
                    .padding([4, 10]),
            ]
            .align_y(Alignment::Center),
            scrollable(
                container(text(log_text).font(iced::Font::MONOSPACE).size(10))
                    .padding(6)
                    .width(Length::Fill),
            )
            .height(Length::Fill),
        ]
        .spacing(6)
        .height(Length::Fill);

        container(content)
            .padding(10)
            .style(card_style)
            .height(Length::Fill)
            .into()
    }
}

fn enumerate_output_devices() -> Vec<String> {
    match cpal::default_host().output_devices() {
        Ok(devs) => {
            let mut devices: Vec<String> = devs.filter_map(|d| d.name().ok()).collect();
            devices.sort();
            devices
        }
        Err(_) => vec!["(could not enumerate devices)".to_string()],
    }
}

fn card_style(theme: &Theme) -> container::Style {
    let palette = theme.palette();
    container::Style::default()
        .background(darken_color(palette.background, 0.05))
        .border(
            Border {
                color: darken_color(palette.background, 0.2),
                width: 1.0,
                radius: 4.0.into(),
            }
        )
}

fn darken_color(color: Color, amount: f32) -> Color {
    Color::from_rgb(
        (color.r * (1.0 - amount)).max(0.0),
        (color.g * (1.0 - amount)).max(0.0),
        (color.b * (1.0 - amount)).max(0.0),
    )
}
