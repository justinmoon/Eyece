mod eye;
mod model;

use std::collections::VecDeque;

use iced::widget::image::Handle as ImageHandle;
use iced::{
    button, executor, pick_list, scrollable, slider, Application, Button, Checkbox, Column,
    Command, Element, Image, Length, PickList, Row, Scrollable, Settings, Slider, Subscription,
    Text,
};
use image::DynamicImage;

fn main() {
    Eyece::run(Settings::default())
}

//#[derive(Default)]
struct Eyece {
    connection: Option<eye::Connection>,
    image: Option<ImageHandle>,
    qr: Option<DynamicImage>,

    config: Config,
    controls: Controls,
    log: Log,
}

impl Default for Eyece {
    fn default() -> Self {
        Self {
            connection: None,
            image: None,
            qr: None,
            config: Config::default(),
            controls: Controls::default(),
            log: Log::default(),
        }
    }
}

#[derive(Debug)]
enum Message {
    EnumDevices,
    DeviceSelected(model::device::Device),
    FormatSelected(model::format::Format),
    ControlChanged(model::control::Control),
    ConfigMessage(ConfigMessage),
    ControlsMessage(ControlsMessage),
    LogMessage(LogMessage),
    ConnectionEvent(eye::subscription::Event),
}

impl Application for Eyece {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        // Perform initial device enumeration.
        // TODO: Async?
        let devices: Vec<model::device::Device> = eye::Device::enumerate()
            .iter()
            .map(|dev| model::device::Device::from(dev.as_str()))
            .collect();

        let mut eyece = Eyece {
            ..Default::default()
        };

        eyece.config.devices = devices;
        eyece.log.level = model::log::Level::Warn;

        (eyece, Command::none())
    }

    fn title(&self) -> String {
        String::from("Eyece")
    }

    fn subscription(&self) -> Subscription<Message> {
        match &self.config.device {
            Some(dev) => iced::Subscription::from_recipe(eye::Subscription::new(&dev.uri))
                .map(Message::ConnectionEvent),
            None => Subscription::none(),
        }
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::EnumDevices => {
                self.config.devices = eye::Device::enumerate()
                    .iter()
                    .map(|dev| model::device::Device::from(dev.as_str()))
                    .collect();
                self.log.update(LogMessage::Log(
                    model::log::Level::Info,
                    format!(
                        "Message::EnumDevices: Found {} devices",
                        self.config.devices.len()
                    ),
                ));
            }
            Message::DeviceSelected(dev) => {
                self.config.device = Some(dev);
            }
            Message::FormatSelected(fmt) => match &self.connection {
                Some(connection) => {
                    connection.set_format(&fmt);
                }
                None => {
                    self.log.update(LogMessage::Log(
                        model::log::Level::Warn,
                        format!("Message::FormatSelected: No connection"),
                    ));
                }
            },
            Message::ControlChanged(control) => match &self.connection {
                Some(connection) => {
                    connection.set_control(&control);
                }
                None => {
                    self.log.update(LogMessage::Log(
                        model::log::Level::Warn,
                        format!("Message::ControlChanged: No connection"),
                    ));
                }
            },
            Message::ConfigMessage(msg) => {
                for msg in self.config.update(msg) {
                    self.update(msg);
                }
            }
            Message::ControlsMessage(msg) => {
                for msg in self.controls.update(msg) {
                    self.update(msg);
                }
            }
            Message::LogMessage(msg) => {
                self.log.update(msg);
            }
            Message::ConnectionEvent(event) => match event {
                eye::subscription::Event::Error(err) => {
                    self.config.device = None;
                    self.controls = Controls::default();
                    self.log.update(LogMessage::Log(
                        model::log::Level::Warn,
                        format!("Event::Error: {}", err),
                    ));
                }
                eye::subscription::Event::Connected(connection) => {
                    connection.query_formats();
                    connection.query_controls();
                    connection.start_stream();
                    self.connection = Some(connection);
                }
                eye::subscription::Event::Disconnected => {
                    self.connection = None;
                }
                eye::subscription::Event::Response(res) => match res {
                    eye::connection::Response::QueryFormats(res) => match res {
                        Ok(formats) => self.config.formats = formats,
                        Err(e) => {
                            self.log.update(LogMessage::Log(
                                model::log::Level::Warn,
                                format!("Event::Response: QueryFormats: Error: {}", e),
                            ));
                        }
                    },
                    eye::connection::Response::QueryControls(res) => match res {
                        Ok(controls) => {
                            self.controls.controls = controls
                                .iter()
                                .map(|model| {
                                    let state = match &model.representation {
                                        model::control::Representation::Button => {
                                            Some(ControlState::Button(button::State::default()))
                                        }
                                        model::control::Representation::Boolean => None,
                                        model::control::Representation::Integer { .. } => {
                                            Some(ControlState::Slider(slider::State::default()))
                                        }
                                        _ => None,
                                    };

                                    (model.clone(), state)
                                })
                                .collect();
                        }
                        Err(e) => {
                            self.log.update(LogMessage::Log(
                                model::log::Level::Warn,
                                format!("Event::Response: QueryControls: Error: {}", e),
                            ));
                        }
                    },
                    eye::connection::Response::StartStream(res) => {
                        if let Err(e) = res {
                            self.log.update(LogMessage::Log(
                                model::log::Level::Warn,
                                format!("Event::StartStream: Error: {}", e),
                            ));
                        }
                    }
                    eye::connection::Response::StopStream(res) => {
                        if let Err(e) = res {
                            self.log.update(LogMessage::Log(
                                model::log::Level::Warn,
                                format!("Event::StopStream: Error: {}", e),
                            ));
                        }
                    }
                    eye::connection::Response::GetFormat(res) => match res {
                        Ok(fmt) => {
                            self.config.format = Some(fmt);
                        }
                        Err(e) => {
                            self.config.format = None;
                            self.log.update(LogMessage::Log(
                                model::log::Level::Warn,
                                format!("Event::GetFormat: Error: {}", e),
                            ))
                        }
                    },
                    eye::connection::Response::SetFormat(res) => match res {
                        Ok(fmt) => {
                            self.config.format = Some(fmt);
                        }
                        Err(e) => {
                            self.config.format = None;
                            self.log.update(LogMessage::Log(
                                model::log::Level::Warn,
                                format!("Event::SetFormat: Error: {}", e),
                            ))
                        }
                    },
                    eye::connection::Response::SetControl(res) => match res {
                        Ok(ctrl) => {
                            for control in &mut self.controls.controls {
                                if control.0.id == ctrl.id {
                                    control.0.value = ctrl.value.clone();
                                }
                            }
                        }
                        Err(e) => self.log.update(LogMessage::Log(
                            model::log::Level::Warn,
                            format!("Event::SetControl: Error: {}", e),
                        )),
                    },
                },
                eye::subscription::Event::Stream((handle, qr)) => {
                    self.image = Some(handle.clone());
                    self.qr = qr.clone();
                }
            },
        }

        Command::none()
    }

    fn view(&mut self) -> Element<Message> {
        // Uniform padding and spacing for all elements.
        const SPACING: u16 = 10;
        const PADDING: u16 = 10;

        let mut image = Row::new();
        if let Some(handle) = &self.image {
            image = image.push(Image::new(handle.clone()));
        }

        Column::new()
            .padding(PADDING)
            .push(self.config.view().map(|msg| Message::ConfigMessage(msg)))
            .push(
                Row::new().spacing(SPACING).push(image).push(
                    self.controls
                        .view()
                        .map(|msg| Message::ControlsMessage(msg)),
                ),
            )
            .push(self.log.view().map(|msg| Message::LogMessage(msg)))
            .into()
    }
}

#[derive(Debug, Default, Clone)]
struct Config {
    devices: Vec<model::device::Device>,
    device: Option<model::device::Device>,
    device_list: pick_list::State<model::device::Device>,
    device_list_refresh: button::State,

    formats: Vec<model::format::Format>,
    format: Option<model::format::Format>,
    format_list: pick_list::State<model::format::Format>,
}

#[derive(Debug, Clone)]
enum ConfigMessage {
    EnumDevices,
    DeviceSelected(model::device::Device),
    FormatSelected(model::format::Format),
}

impl Config {
    fn update(&mut self, message: ConfigMessage) -> Vec<Message> {
        match message {
            ConfigMessage::EnumDevices => vec![Message::EnumDevices],
            ConfigMessage::DeviceSelected(dev) => vec![
                Message::LogMessage(LogMessage::Log(
                    model::log::Level::Info,
                    format!("ConfigMessage::DeviceSelected: {}", dev.uri),
                )),
                Message::DeviceSelected(dev),
            ],
            ConfigMessage::FormatSelected(fmt) => vec![
                Message::LogMessage(LogMessage::Log(
                    model::log::Level::Info,
                    format!(
                        "ConfigMessage::FormatSelected: {}x{}",
                        fmt.width, fmt.height
                    ),
                )),
                Message::FormatSelected(fmt),
            ],
        }
    }

    fn view(&mut self) -> Element<ConfigMessage> {
        // Uniform padding and spacing for all elements.
        const PADDING: u16 = 10;

        // Device selection, format configuration, etc.
        Row::new()
            .padding(PADDING)
            .push(
                Button::new(&mut self.device_list_refresh, Text::new("Refresh"))
                    .on_press(ConfigMessage::EnumDevices),
            )
            .push(PickList::new(
                &mut self.device_list,
                &self.devices,
                self.device.clone(),
                ConfigMessage::DeviceSelected,
            ))
            .push(PickList::new(
                &mut self.format_list,
                &self.formats,
                self.format.clone(),
                ConfigMessage::FormatSelected,
            ))
            .into()
    }
}

#[derive(Debug, Default, Clone)]
struct Controls {
    state: scrollable::State,
    controls: Vec<(model::control::Control, Option<ControlState>)>,
}

#[derive(Debug, Clone)]
enum ControlState {
    Button(button::State),
    Slider(slider::State),
}

#[derive(Debug, Clone)]
enum ControlsMessage {
    ControlChanged(model::control::Control),
}

impl Controls {
    fn update(&mut self, message: ControlsMessage) -> Vec<Message> {
        match message {
            ControlsMessage::ControlChanged(ctrl) => vec![
                Message::LogMessage(LogMessage::Log(
                    model::log::Level::Info,
                    format!("ControlsMessage::ControlChanged: ID: {}", ctrl.id),
                )),
                Message::ControlChanged(ctrl),
            ],
        }
    }

    fn view(&mut self) -> Element<ControlsMessage> {
        // Uniform padding and spacing for all elements.
        const SPACING: u16 = 10;
        const PADDING: u16 = 10;

        // Device controls
        let mut controls = Scrollable::new(&mut self.state).width(Length::Fill);
        for (control, state) in &mut self.controls {
            let control_clone = control.clone();

            match &control.representation {
                model::control::Representation::Button => {
                    let state = match state.as_mut().unwrap() {
                        ControlState::Button(state) => state,
                        _ => panic!("Wrong button state"),
                    };
                    controls = controls.push(
                        Row::new()
                            .spacing(SPACING)
                            .push(Text::new(control.name.clone()))
                            .push(
                                Button::new(state, Text::new("Toggle"))
                                    .on_press(ControlsMessage::ControlChanged(control_clone)),
                            ),
                    );
                }
                model::control::Representation::Boolean => {
                    let checked = match control.value {
                        model::control::Value::Boolean(val) => Some(val),
                        model::control::Value::Integer(val) => Some(val != 0),
                        _ => None,
                    };
                    controls = controls.push(
                        Row::new()
                            .spacing(SPACING)
                            .push(Text::new(control.name.clone()))
                            .push(Checkbox::new(checked.unwrap(), "", move |val| {
                                let mut control = control_clone.clone();
                                control.value = model::control::Value::Boolean(val);
                                ControlsMessage::ControlChanged(control)
                            })),
                    );
                }
                model::control::Representation::Integer {
                    range,
                    step,
                    default: _,
                } => {
                    let state = match state.as_mut().unwrap() {
                        ControlState::Slider(state) => state,
                        _ => panic!("Wrong slider state"),
                    };
                    let value = match control.value {
                        model::control::Value::Integer(val) => Some(val),
                        _ => None,
                    };
                    controls = controls.push(
                        Row::new()
                            .spacing(SPACING)
                            .push(Text::new(control.name.clone()))
                            .push(
                                Slider::new(
                                    state,
                                    (range.0 as f64)..=(range.1 as f64),
                                    value.unwrap() as f64,
                                    move |val| {
                                        let mut control = control_clone.clone();
                                        control.value = model::control::Value::Integer(val as i64);
                                        ControlsMessage::ControlChanged(control)
                                    },
                                )
                                .step(*step as f64),
                            ),
                    );
                }
                _ => continue,
            }
        }

        Column::new().padding(PADDING).push(controls).into()
    }
}

#[derive(Debug, Default, Clone)]
struct Log {
    state: scrollable::State,
    level: model::log::Level,
    level_list: pick_list::State<model::log::Level>,
    buffer: VecDeque<(model::log::Level, String)>,
}

#[derive(Debug, Clone)]
enum LogMessage {
    Log(model::log::Level, String),
    LevelSelected(model::log::Level),
}

impl Log {
    fn update(&mut self, message: LogMessage) {
        match message {
            LogMessage::Log(level, message) => {
                if self.buffer.len() > 100 {
                    self.buffer.pop_front();
                }
                self.buffer.push_back((level, message));
            }
            LogMessage::LevelSelected(level) => {
                self.level = level;
            }
        }
    }

    fn view(&mut self) -> Element<LogMessage> {
        // Uniform padding and spacing for all elements.
        const SPACING: u16 = 10;
        const PADDING: u16 = 10;

        let settings = Row::new().push(PickList::new(
            &mut self.level_list,
            &model::log::Level::ALL[..],
            Some(self.level),
            LogMessage::LevelSelected,
        ));

        let mut logs = Scrollable::new(&mut self.state)
            .width(Length::Fill)
            .height(Length::Units(100));

        for entry in &self.buffer {
            if entry.0 as u8 <= self.level as u8 {
                logs = logs.push(
                    Row::new()
                        .spacing(SPACING)
                        .push(Text::new(format!("[{}]", entry.0)))
                        .push(Text::new(entry.1.clone())),
                );
            }
        }

        Column::new()
            .padding(PADDING)
            .push(settings)
            .push(logs)
            .into()
    }
}
