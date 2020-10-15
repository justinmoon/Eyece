use std::{fmt, io, sync::mpsc};

use bardecoder;
use image;
use image::DynamicImage;

use eye::device::Device as Dev;
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
    uri: String,
}

impl Subscription {
    pub fn new<S: Into<String>>(uri: S) -> Self {
        Subscription { uri: uri.into() }
    }
}

fn decode_qr(frame: &ImageView, pixels: &[u8]) -> Option<String> {
    //let chars: Vec<u8> = pixels.into_iter().take(20).collect();
    let img: image::ImageBuffer<image::Bgra<_>, Vec<u8>> =
        image::ImageBuffer::from_raw(frame.width(), frame.height(), pixels.to_vec()).unwrap();
    let img = DynamicImage::ImageBgra8(img);
    let decoder = bardecoder::default_decoder();
    let results = decoder.decode(&img);
    for result in results {
        // FIXME: combinator
        match result {
            Ok(r) => return Some(r),
            Err(_) => {
                println!("scan error");
                return None;
            }
        }
    }
    None
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
                                resolutions.push(model::format::Format {
                                    width: info.width,
                                    height: info.height,
                                });
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
                                | model::control::Representation::Integer { .. } => {
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
            Request::GetFormat => {
                let res = device.format();
                match res {
                    Ok(fmt) => Some(Response::GetFormat(Ok(model::format::Format {
                        width: fmt.width,
                        height: fmt.height,
                    }))),
                    Err(e) => Some(Response::GetFormat(Err(e))),
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
        self.uri.hash(state);
    }

    fn stream(
        self: Box<Self>,
        _input: futures::stream::BoxStream<'static, I>,
    ) -> futures::stream::BoxStream<'static, Self::Output> {
        Box::pin(futures::stream::unfold(
            State::Ready(self.uri),
            |state| async move {
                match state {
                    State::Ready(uri) => {
                        let (tx, rx) = mpsc::channel();
                        let connection = Connection::new(tx);

                        // open the device
                        let mut device = match Dev::with_uri(&uri) {
                            Ok(device) => device,
                            Err(e) => return Some((Event::Error(e), State::Finished)),
                        };

                        // read the current format
                        let mut format = match device.format() {
                            Ok(format) => format,
                            Err(e) => return Some((Event::Error(e), State::Finished)),
                        };

                        // Iced only supports BGRA images, so request that exact format.
                        // Eye-rs will transparently convert the images on-the-fly if necessary
                        // (and possible).
                        format.pixfmt = PixelFormat::Bgra(32);

                        // set the new format
                        let format = match device.set_format(&format) {
                            Ok(format) => format,
                            Err(e) => return Some((Event::Error(e), State::Finished)),
                        };

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
                                    pixels.clone(),
                                );
                                let qr = decode_qr(&frame, &pixels);

                                println!("qr: {:?}", qr);
                                Some((
                                    Event::Stream((handle, qr)),
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

pub enum Event {
    Error(io::Error),
    Connected(Connection),
    Disconnected,
    Response(Response),
    Stream((iced::image::Handle, Option<String>)),
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Event::Stream(_) => write!(f, "stream"),
            Event::Error(_) => write!(f, "error"),
            Event::Connected(_) => write!(f, "connected"),
            Event::Disconnected => write!(f, "Disconnected"),
            Event::Response(_) => write!(f, "Response"),
        }
    }
}

enum State<'a> {
    Ready(String),
    Idle {
        comm: mpsc::Receiver<Request>,
        device: SendWrapper<Box<dyn Device>>,
    },
    Streaming {
        comm: mpsc::Receiver<Request>,
        device: SendWrapper<Box<dyn Device>>,
        stream: SendWrapper<Box<dyn 'a + for<'b> Stream<'b, Item = ImageView<'b>>>>,
    },
    Finished,
}
