// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use carnelian::app::{Config, ViewCreationParameters};
use carnelian::color::Color;
use carnelian::drawing::{load_font, path_for_rectangle, path_for_rounded_rectangle, FontFace};
use carnelian::input::{self};
use carnelian::render::{BlendMode, Context as RenderContext, Fill, FillRule, Layer, Path, Style};
use carnelian::scene::facets::{Facet, FacetId, TextFacetOptions};
use carnelian::scene::scene::{Scene, SceneBuilder, SceneOrder};
use carnelian::scene::LayerGroup;
use carnelian::{
    derive_handle_message_with_default, App, AppAssistant, AppAssistantPtr, AppSender,
    AssistantCreatorFunc, Coord, LocalBoxFuture, MessageTarget, Point, Rect, Size, ViewAssistant,
    ViewAssistantContext, ViewAssistantPtr, ViewKey,
};
use euclid::{point2, size2, vec2, Angle, Transform2D};
use fidl::prelude::*;
use fidl_test_placeholders::{EchoMarker, EchoRequest, EchoRequestStream};
use fuchsia_async as fasync;
use fuchsia_zircon::Time;
use futures::prelude::*;
use std::f32::consts::PI;
use std::path::PathBuf;

struct SpinningSquareAppAssistant {
    app_sender: AppSender,
}

impl SpinningSquareAppAssistant {
    fn new(app_sender: AppSender) -> Self {
        Self { app_sender }
    }
}

impl AppAssistant for SpinningSquareAppAssistant {
    fn setup(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn create_view_assistant_with_parameters(
        &mut self,
        params: ViewCreationParameters,
    ) -> Result<ViewAssistantPtr, Error> {
        let additional = params.options.is_some();
        let direction = params
            .options
            .and_then(|options| options.downcast_ref::<Direction>().map(|direction| *direction))
            .unwrap_or(Direction::CounterClockwise);
        SpinningSquareViewAssistant::new(
            params.view_key,
            direction,
            self.app_sender.clone(),
            additional,
        )
    }

    /// Return the list of names of services this app wants to provide
    fn outgoing_services_names(&self) -> Vec<&'static str> {
        [EchoMarker::PROTOCOL_NAME].to_vec()
    }

    /// Handle a request to connect to a service provided by this app
    fn handle_service_connection_request(
        &mut self,
        _service_name: &str,
        channel: fasync::Channel,
    ) -> Result<(), Error> {
        Self::create_echo_server(channel, false);
        Ok(())
    }

    fn filter_config(&mut self, config: &mut Config) {
        config.display_resource_release_delay = std::time::Duration::new(0, 0);
    }
}

impl SpinningSquareAppAssistant {
    fn create_echo_server(channel: fasync::Channel, quiet: bool) {
        fasync::Task::local(
            async move {
                let mut stream = EchoRequestStream::from_channel(channel);
                while let Some(EchoRequest::EchoString { value, responder }) =
                    stream.try_next().await.context("error running echo server")?
                {
                    if !quiet {
                        println!("Spinning Square received echo request for string {:?}", value);
                    }
                    responder
                        .send(value.as_ref().map(|s| &**s))
                        .context("error sending response")?;
                    if !quiet {
                        println!("echo response sent successfully");
                    }
                }
                Ok(())
            }
            .unwrap_or_else(|e: anyhow::Error| eprintln!("{:?}", e)),
        )
        .detach();
    }
}

struct SceneDetails {
    scene: Scene,
    square: FacetId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Clockwise,
    CounterClockwise,
}

impl Direction {
    pub fn toggle(self) -> Self {
        match self {
            Self::Clockwise => Self::CounterClockwise,
            Self::CounterClockwise => Self::Clockwise,
        }
    }
}

#[derive(Debug)]
pub struct ToggleRoundedMessage {}

#[derive(Debug)]
pub struct ToggleDirectionMessage {}

struct SpinningSquareFacet {
    direction: Direction,
    square_color: Color,
    rounded: bool,
    start: Time,
    square_path: Option<Path>,
    size: Size,
}

impl SpinningSquareFacet {
    fn new(square_color: Color, start: Time, size: Size, direction: Direction) -> Self {
        Self { direction, square_color, rounded: false, start, square_path: None, size }
    }

    fn clone_square_path(&self) -> Path {
        self.square_path.as_ref().expect("square_path").clone()
    }

    fn handle_toggle_rounded_message(&mut self, _msg: &ToggleRoundedMessage) {
        self.rounded = !self.rounded;
        self.square_path = None;
    }

    fn handle_toggle_direction_message(&mut self, _msg: &ToggleDirectionMessage) {
        self.direction = self.direction.toggle();
    }

    fn handle_other_message(&mut self, _msg: &carnelian::Message) {
        println!("handle_other_message");
    }
}

impl Facet for SpinningSquareFacet {
    fn update_layers(
        &mut self,
        size: Size,
        layer_group: &mut dyn LayerGroup,
        render_context: &mut RenderContext,
        view_context: &ViewAssistantContext,
    ) -> Result<(), Error> {
        const SPEED: f32 = 0.25;
        const SECONDS_PER_NANOSECOND: f32 = 1e-9;
        const SQUARE_PATH_SIZE: Coord = 1.0;
        const SQUARE_PATH_SIZE_2: Coord = SQUARE_PATH_SIZE / 2.0;
        const CORNER_RADIUS: Coord = SQUARE_PATH_SIZE / 4.0;

        let center_x = size.width * 0.5;
        let center_y = size.height * 0.5;
        self.size = size;
        let square_size = size.width.min(size.height) * 0.6;
        let presentation_time = view_context.presentation_time;
        let t = ((presentation_time.into_nanos() - self.start.into_nanos()) as f32
            * SECONDS_PER_NANOSECOND
            * SPEED)
            % 1.0;
        let angle =
            t * PI * 2.0 * if self.direction == Direction::CounterClockwise { -1.0 } else { 1.0 };

        if self.square_path.is_none() {
            let top_left = point2(-SQUARE_PATH_SIZE_2, -SQUARE_PATH_SIZE_2);
            let square = Rect::new(top_left, size2(SQUARE_PATH_SIZE, SQUARE_PATH_SIZE));
            let square_path = if self.rounded {
                path_for_rounded_rectangle(&square, CORNER_RADIUS, render_context)
            } else {
                path_for_rectangle(&square, render_context)
            };
            self.square_path.replace(square_path);
        }

        let transformation = Transform2D::rotation(Angle::radians(angle))
            .then_scale(square_size, square_size)
            .then_translate(vec2(center_x, center_y));
        let mut raster_builder = render_context.raster_builder().expect("raster_builder");
        raster_builder.add(&self.clone_square_path(), Some(&transformation));
        let square_raster = raster_builder.build();

        layer_group.insert(
            SceneOrder::default(),
            Layer {
                raster: square_raster,
                clip: None,
                style: Style {
                    fill_rule: FillRule::NonZero,
                    fill: Fill::Solid(self.square_color),
                    blend_mode: BlendMode::Over,
                },
            },
        );
        Ok(())
    }

    derive_handle_message_with_default!(handle_other_message,
        ToggleRoundedMessage => handle_toggle_rounded_message,
        ToggleDirectionMessage => handle_toggle_direction_message
    );

    fn calculate_size(&self, _available: Size) -> Size {
        self.size
    }
}

struct SpinningSquareViewAssistant {
    direction: Direction,
    view_key: ViewKey,
    background_color: Color,
    square_color: Color,
    start: Time,
    app_sender: AppSender,
    scene_details: Option<SceneDetails>,
    face: FontFace,
    additional: bool,
}

impl SpinningSquareViewAssistant {
    fn new(
        view_key: ViewKey,
        direction: Direction,
        app_sender: AppSender,
        additional: bool,
    ) -> Result<ViewAssistantPtr, Error> {
        let square_color = Color { r: 0xbb, g: 0x00, b: 0xff, a: 0xbb };
        let background_color = Color { r: 0x3f, g: 0x8a, b: 0x99, a: 0xff };
        let start = Time::get_monotonic();
        let face = load_font(PathBuf::from("/pkg/data/fonts/RobotoSlab-Regular.ttf"))?;

        Ok(Box::new(SpinningSquareViewAssistant {
            direction,
            view_key,
            background_color,
            square_color,
            start,
            scene_details: None,
            app_sender,
            face,
            additional,
        }))
    }

    fn ensure_scene_built(&mut self, size: Size) {
        if self.scene_details.is_none() {
            let min_dimension = size.width.min(size.height);
            let font_size = (min_dimension / 5.0).ceil().min(64.0);
            let mut builder =
                SceneBuilder::new().background_color(self.background_color).animated(true);
            let mut square = None;
            builder.group().stack().center().contents(|builder| {
                if self.additional {
                    let key_text = format!("{}", self.view_key);
                    let _ = builder.text(
                        self.face.clone(),
                        &key_text,
                        font_size,
                        Point::zero(),
                        TextFacetOptions::default(),
                    );
                }
                let square_facet =
                    SpinningSquareFacet::new(self.square_color, self.start, size, self.direction);
                square = Some(builder.facet(Box::new(square_facet)));
                const STRIPE_COUNT: usize = 5;
                let stripe_height = size.height / (STRIPE_COUNT * 2 + 1) as f32;
                const STRIPE_WIDTH_RATIO: f32 = 0.8;
                let stripe_size = size2(size.width * STRIPE_WIDTH_RATIO, stripe_height);
                builder.group().column().max_size().space_evenly().contents(|builder| {
                    for _ in 0..STRIPE_COUNT {
                        builder.rectangle(stripe_size, Color::white());
                    }
                });
            });
            let square = square.expect("square");
            let scene = builder.build();
            self.scene_details = Some(SceneDetails { scene, square });
        }
    }

    fn toggle_rounded(&mut self) {
        if let Some(scene_details) = self.scene_details.as_mut() {
            // since we have the scene, we could call send_message directly,
            // but this lets us demonstrate facet-targeted messages.
            self.app_sender.queue_message(
                MessageTarget::Facet(self.view_key, scene_details.square),
                Box::new(ToggleRoundedMessage {}),
            );
            self.app_sender.request_render(self.view_key);
        }
    }

    fn move_backward(&mut self) {
        if let Some(scene_details) = self.scene_details.as_mut() {
            scene_details
                .scene
                .move_facet_backward(scene_details.square)
                .unwrap_or_else(|e| println!("error in move_facet_backward: {}", e));
            self.app_sender.request_render(self.view_key);
        }
    }

    fn move_forward(&mut self) {
        if let Some(scene_details) = self.scene_details.as_mut() {
            scene_details
                .scene
                .move_facet_forward(scene_details.square)
                .unwrap_or_else(|e| println!("error in move_facet_forward: {}", e));
            self.app_sender.request_render(self.view_key);
        }
    }

    fn toggle_direction(&mut self) {
        if let Some(scene_details) = self.scene_details.as_mut() {
            self.app_sender.queue_message(
                MessageTarget::Facet(self.view_key, scene_details.square),
                Box::new(ToggleDirectionMessage {}),
            );
            self.app_sender.request_render(self.view_key);
        }
    }

    fn make_new_view(&mut self) {
        let direction = self.direction.toggle();
        self.app_sender.create_additional_view(Some(Box::new(direction)));
    }

    fn close_additional_view(&mut self) {
        if self.additional {
            self.app_sender.close_additional_view(self.view_key);
        } else {
            println!("Cannot close initial window");
        }
    }
}

impl ViewAssistant for SpinningSquareViewAssistant {
    fn resize(&mut self, new_size: &Size) -> Result<(), Error> {
        self.scene_details = None;
        self.ensure_scene_built(*new_size);
        Ok(())
    }

    fn get_scene(&mut self, size: Size) -> Option<&mut Scene> {
        self.ensure_scene_built(size);
        Some(&mut self.scene_details.as_mut().unwrap().scene)
    }

    fn handle_keyboard_event(
        &mut self,
        _context: &mut ViewAssistantContext,
        _event: &input::Event,
        keyboard_event: &input::keyboard::Event,
    ) -> Result<(), Error> {
        const SPACE: u32 = ' ' as u32;
        const B: u32 = 'b' as u32;
        const F: u32 = 'f' as u32;
        const D: u32 = 'd' as u32;
        const V: u32 = 'v' as u32;
        const C: u32 = 'c' as u32;
        if let Some(code_point) = keyboard_event.code_point {
            if keyboard_event.phase == input::keyboard::Phase::Pressed
                || keyboard_event.phase == input::keyboard::Phase::Repeat
            {
                match code_point {
                    SPACE => self.toggle_rounded(),
                    B => self.move_backward(),
                    F => self.move_forward(),
                    D => self.toggle_direction(),
                    V => self.make_new_view(),
                    C => self.close_additional_view(),
                    _ => println!("code_point = {}", code_point),
                }
            }
        }
        Ok(())
    }
}

fn make_app_assistant_fut(
    app_sender: &AppSender,
) -> LocalBoxFuture<'_, Result<AppAssistantPtr, Error>> {
    let f = async move {
        let assistant = Box::new(SpinningSquareAppAssistant::new(app_sender.clone()));
        Ok::<AppAssistantPtr, Error>(assistant)
    };
    Box::pin(f)
}

fn make_app_assistant() -> AssistantCreatorFunc {
    Box::new(make_app_assistant_fut)
}

fn main() -> Result<(), Error> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    App::run(make_app_assistant())
}
