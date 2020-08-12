use std::{io, sync::mpsc};

use eye::hal::traits::{Device, Stream};
use eye::prelude::*;

use ffimage::packed::dynamic::ImageView;

use iced_futures::futures;

use crate::eye::{
    connection::{Connection, Request, Response},
    util::SendWrapper,
};
use crate::model;

pub struct Subscription {
    index: usize,
}

impl Subscription {
    pub fn new(index: usize) -> Self {
        Subscription { index }
    }
}

impl Subscription {
    fn handle_request(device: &mut Box<dyn Device>, request: Request) -> Option<Response> {
        match request {
            Request::QueryFormats => {
                let res = device.query_formats();
                match res {
                    Ok(info) => {
                        let mut resolutions = Vec::new();
                        for info in info {
                            if info.pixfmt == eye::format::PixelFormat::Bgra(24) {
                                for res in info.resolutions {
                                    resolutions.push(model::format::Format {
                                        width: res.0,
                                        height: res.1,
                                    });
                                }
                            }
                        }

                        Some(Response::QueryFormats(Ok(resolutions)))
                    }
                    Err(e) => Some(Response::QueryFormats(Err(e))),
                }
            }
            Request::QueryControls => {
                let res = device.query_controls();
                match res {
                    Ok(info) => {
                        let mut controls: Vec<model::control::Control> = info
                            .iter()
                            .map(|ctrl| model::control::Control::from(ctrl))
                            .collect();

                        // query control values
                        for control in &mut controls {
                            match &control.representation {
                                model::control::Representation::Boolean
                                | model::control::Representation::Integer(_) => {
                                    let res = device.control(control.id);
                                    if let Ok(val) = res {
                                        control.value = val;
                                    }
                                }
                                _ => continue,
                            }
                        }

                        Some(Response::QueryControls(Ok(controls)))
                    }
                    Err(e) => Some(Response::QueryControls(Err(e))),
                }
            }
            Request::SetFormat(fmt) => {
                let mut res = device.format();
                if let Ok(format) = &mut res {
                    format.width = fmt.width;
                    format.height = fmt.height;
                    res = device.set_format(&format);
                }
                match res {
                    Ok(fmt) => Some(Response::SetFormat(Ok(model::format::Format {
                        width: fmt.width,
                        height: fmt.height,
                    }))),
                    Err(e) => Some(Response::SetFormat(Err(e))),
                }
            }
            Request::SetControl(ctrl) => {
                let res = device.set_control(ctrl.id, &ctrl.value);
                match res {
                    Ok(()) => Some(Response::SetControl(Ok(ctrl))),
                    Err(e) => Some(Response::SetControl(Err(e))),
                }
            }
            _ => None,
        }
    }
}

impl<H, I> iced_futures::subscription::Recipe<H, I> for Subscription
where
    H: std::hash::Hasher,
{
    type Output = Event;

    fn hash(&self, state: &mut H) {
        use std::hash::Hash;

        std::any::TypeId::of::<Self>().hash(state);
        self.index.hash(state);
    }

    fn stream(
        self: Box<Self>,
        _input: futures::stream::BoxStream<'static, I>,
    ) -> futures::stream::BoxStream<'static, Self::Output> {
        Box::pin(futures::stream::unfold(
            State::Ready(self.index),
            |state| async move {
                match state {
                    State::Ready(index) => {
                        let (tx, rx) = mpsc::channel();
                        let connection = Connection::new(tx);

                        // open the device
                        let res = DeviceFactory::create(index as usize);
                        if res.is_err() {
                            return Some((Event::Error(res.err().unwrap()), State::Finished));
                        }
                        let mut device = res.unwrap();

                        // read the current format
                        let res = device.format();
                        if res.is_err() {
                            return Some((Event::Error(res.err().unwrap()), State::Finished));
                        }
                        let mut format = res.unwrap();

                        // Iced only supports BGRA images, so request that exact format.
                        // Eye-rs will transparently convert the images on-the-fly if necessary
                        // (and possible).
                        format.pixfmt = PixelFormat::Bgra(32);

                        // set the new format
                        let res = device.set_format(&format);
                        if res.is_err() {
                            return Some((Event::Error(res.err().unwrap()), State::Finished));
                        }
                        let format = res.unwrap();

                        if format.pixfmt != PixelFormat::Bgra(32) {
                            let err = io::Error::new(
                                io::ErrorKind::InvalidData,
                                "device does not support BGRA capture",
                            );
                            return Some((Event::Error(err), State::Finished));
                        }

                        Some((
                            Event::Connected(connection),
                            State::Idle {
                                comm: rx,
                                device: unsafe { SendWrapper::new(device) },
                            },
                        ))
                    }
                    State::Idle { comm, mut device } => {
                        let request;
                        match comm.recv() {
                            Ok(req) => request = req,
                            Err(_) => {
                                // The other side hung up, there's nothing left to do.
                                return Some((Event::Disconnected, State::Finished));
                            }
                        }

                        match request {
                            Request::StartStream => {
                                let res = device.stream();
                                match res {
                                    Ok(stream) => Some((
                                        Event::Response(Response::StartStream(Ok(()))),
                                        State::Streaming {
                                            comm,
                                            device,
                                            stream: unsafe { SendWrapper::new(stream) },
                                        },
                                    )),
                                    Err(e) => Some((
                                        Event::Response(Response::StartStream(Err(e))),
                                        State::Idle { comm, device },
                                    )),
                                }
                            }
                            Request::QueryFormats
                            | Request::QueryControls
                            | Request::SetFormat(..)
                            | Request::SetControl(..) => {
                                let event = match Self::handle_request(&mut *device, request) {
                                    Some(res) => Event::Response(res),
                                    None => Event::Error(io::Error::new(
                                        io::ErrorKind::InvalidInput,
                                        "cannot handle request",
                                    )),
                                };

                                Some((event, State::Idle { comm, device }))
                            }
                            _ => Some((
                                Event::Error(io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "cannot handle this request in idle state",
                                )),
                                State::Idle { comm, device },
                            )),
                        }
                    }
                    State::Streaming {
                        comm,
                        mut device,
                        mut stream,
                    } => {
                        match comm.try_recv() {
                            Ok(req) => match req {
                                Request::StopStream => {
                                    return Some((
                                        Event::Response(Response::StopStream(Ok(()))),
                                        State::Idle { comm, device },
                                    ));
                                }
                                Request::SetFormat(fmt) => {
                                    // We cannot change the format while a stream is currently
                                    // active, so drop it and recreate it on success.
                                    std::mem::drop(stream);

                                    let event = match Self::handle_request(
                                        &mut *device,
                                        Request::SetFormat(fmt),
                                    ) {
                                        Some(res) => Event::Response(res),
                                        None => Event::Error(io::Error::new(
                                            io::ErrorKind::InvalidInput,
                                            "cannot handle request",
                                        )),
                                    };

                                    let res = device.stream();
                                    match res {
                                        Ok(stream) => {
                                            return Some((
                                                event,
                                                State::Streaming {
                                                    comm,
                                                    device,
                                                    stream: unsafe { SendWrapper::new(stream) },
                                                },
                                            ));
                                        }
                                        Err(e) => {
                                            return Some((
                                                Event::Response(Response::SetFormat(Err(e))),
                                                State::Idle { comm, device },
                                            ));
                                        }
                                    }
                                }
                                Request::SetControl(ctrl) => {
                                    let event = match Self::handle_request(
                                        &mut *device,
                                        Request::SetControl(ctrl),
                                    ) {
                                        Some(res) => Event::Response(res),
                                        None => Event::Error(io::Error::new(
                                            io::ErrorKind::InvalidInput,
                                            "cannot handle request",
                                        )),
                                    };

                                    return Some((
                                        event,
                                        State::Streaming {
                                            comm,
                                            device,
                                            stream,
                                        },
                                    ));
                                }
                                _ => {
                                    return Some((
                                        Event::Error(io::Error::new(
                                            io::ErrorKind::InvalidInput,
                                            "cannot handle this request in streaming state",
                                        )),
                                        State::Streaming {
                                            comm,
                                            device,
                                            stream,
                                        },
                                    ));
                                }
                            },
                            Err(_) => { /* ignore */ }
                        }

                        match stream.next() {
                            Ok(frame) => {
                                let pixels = frame.raw().as_slice().unwrap().to_vec();
                                let handle = iced::image::Handle::from_pixels(
                                    frame.width(),
                                    frame.height(),
                                    pixels,
                                );
                                Some((
                                    Event::Stream(handle),
                                    State::Streaming {
                                        device,
                                        stream,
                                        comm,
                                    },
                                ))
                            }
                            Err(e) => Some((Event::Error(e), State::Idle { comm, device })),
                        }
                    }
                    State::Finished => {
                        // Let the stream die, just that like that.
                        None
                    }
                }
            },
        ))
    }
}

#[derive(Debug)]
pub enum Event {
    Error(io::Error),
    Connected(Connection),
    Disconnected,
    Response(Response),
    Stream(iced::image::Handle),
}

enum State<'a> {
    Ready(usize),
    Idle {
        comm: mpsc::Receiver<Request>,
        device: SendWrapper<Box<dyn Device>>,
    },
    Streaming {
        comm: mpsc::Receiver<Request>,
        device: SendWrapper<Box<dyn Device>>,
        stream: SendWrapper<Box<dyn Stream<Item = ImageView<'a>>>>,
    },
    Finished,
}
