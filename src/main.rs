//! Renders a 2D scene containing a single, moving sprite.

use std::{borrow::Cow, f32::consts::PI};

use bevy::{
    asset::{AssetLoader, BoxedFuture, LoadContext, LoadedAsset},
    ecs::system::{lifetimeless::SRes, SystemParamItem},
    math::vec3,
    prelude::*,
    reflect::TypeUuid,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::{PrepareAssetError, RenderAsset, RenderAssets},
        render_graph::{self, RenderGraph},
        render_resource::{
            BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
            BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, Buffer,
            BufferBinding, BufferBindingType, BufferDescriptor, BufferInitDescriptor, BufferSize,
            BufferUsages, CachedComputePipelineId, CachedPipelineState, ComputePassDescriptor,
            ComputePipelineDescriptor, PipelineCache, ShaderStages, ShaderType,
            StorageTextureAccess, TextureFormat, TextureUsages, TextureViewDimension,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue},
        Extract, RenderApp, RenderStage,
    },
    window::PresentMode,
};
use bytemuck::{Pod, Zeroable};
use serde::Deserialize;

const NO_SLIMES: u32 = 100;
const WIDTH: f32 = 1280.;
const HEIGHT: f32 = 720.;
const WORKGROUP_SIZE: u32 = 8;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            window: WindowDescriptor {
                title: "Slime Simulation".to_string(),
                width: WIDTH,
                height: HEIGHT,
                present_mode: PresentMode::AutoVsync,
                ..default()
            },
            ..default()
        }))
        .add_plugin(SlimeComputePlugin)
        .add_asset::<Slime>()
        .init_asset_loader::<SlimeLoader>()
        .add_startup_system(setup)
        .add_system(bevy::window::close_on_esc)
        .insert_resource(ClearColor(Color::rgb(0., 0., 0.)))
        .run();
}

#[derive(Debug, Copy, Clone, ShaderType, Default, Resource, TypeUuid, Deserialize)]
#[uuid = "1ebefa44-80b6-46bc-939d-5bf39ff15f53"]
struct Slime {
    pub value: f32,
    pub _padding0: f32,
    pub _padding1: f32,
    pub _padding2: f32,
}
#[derive(Default)]
pub struct SlimeLoader;

impl AssetLoader for SlimeLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> BoxedFuture<'a, Result<(), bevy::asset::Error>> {
        Box::pin(async move {
            let custom_asset = ron::de::from_bytes::<Slime>(bytes)?;
            load_context.set_default_asset(LoadedAsset::new(custom_asset));
            Ok(())
        })
    }

    fn extensions(&self) -> &[&str] {
        &["slime"]
    }
}

unsafe impl Pod for Slime {}
unsafe impl Zeroable for Slime {}

#[derive(Debug, Clone)]
struct GpuSlime {
    pub buffer: Buffer,
}

impl RenderAsset for Slime {
    type ExtractedAsset = Slime;
    type PreparedAsset = GpuSlime;
    type Param = (SRes<RenderDevice>, SRes<RenderQueue>);

    /// Clones the Image.
    fn extract_asset(&self) -> Self::ExtractedAsset {
        self.clone()
    }

    /// Converts the extracted image into a [`GpuImage`].
    fn prepare_asset(
        image: Self::ExtractedAsset,
        (render_device, render_queue): &mut SystemParamItem<Self::Param>,
    ) -> Result<Self::PreparedAsset, PrepareAssetError<Self::ExtractedAsset>> {
        let buffer = render_device.create_buffer(&BufferDescriptor {
            label: None,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            size: 4,
            mapped_at_creation: true,
        });
        render_queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&[image]));

        Ok(GpuSlime { buffer })
    }
}

#[derive(Debug, Clone, Deref, Resource, ExtractResource)]
struct SlimeHandle(Handle<Slime>);

fn setup(mut commands: Commands, mut slimes: ResMut<Assets<Slime>>) {
    commands.spawn(Camera2dBundle::default());
    let slime = slimes.add(Slime::default());
    commands.insert_resource(SlimeHandle(slime));
}

pub struct SlimeComputePlugin;

impl Plugin for SlimeComputePlugin {
    fn build(&self, app: &mut App) {
        // Extract the game of life image resource from the main world into the render world
        // for operation on by the compute shader and display on the sprite.
        //
        app.add_plugin(ExtractResourcePlugin::<SlimeHandle>::default());
        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .init_resource::<SlimePipeline>()
            .add_system_to_stage(RenderStage::Queue, queue_bind_group)
            .add_system_to_stage(RenderStage::Extract, extract_slime);

        let mut render_graph = render_app.world.resource_mut::<RenderGraph>();
        render_graph.add_node("game_of_life", SlimeNode::default());
        render_graph
            .add_node_edge(
                "game_of_life",
                bevy::render::main_graph::node::CAMERA_DRIVER,
            )
            .unwrap();
    }
}

#[derive(Resource)]
struct SlimeBindGroup(BindGroup);

fn extract_slime() {}

fn queue_bind_group(
    mut commands: Commands,
    pipeline: Res<SlimePipeline>,
    render_device: Res<RenderDevice>,
    slime_store: Res<RenderAssets<Slime>>,
    slime: Res<SlimeHandle>,
) {
    error!("Got slime {:?}", slime);
    let slime = &slime_store[&slime.0];

    let bind_group = render_device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.texture_bind_group_layout,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(BufferBinding {
                buffer: &slime.buffer,
                offset: 0,
                size: None,
            }),
        }],
    });
    commands.insert_resource(SlimeBindGroup(bind_group));
}

#[derive(Resource)]
pub struct SlimePipeline {
    texture_bind_group_layout: BindGroupLayout,
    update_pipeline: CachedComputePipelineId,
}

impl FromWorld for SlimePipeline {
    fn from_world(world: &mut World) -> Self {
        let texture_bind_group_layout =
            world
                .resource::<RenderDevice>()
                .create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: None,
                    entries: &[BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::COMPUTE,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: BufferSize::new(
                                (std::mem::size_of::<f32>() * 4) as u64,
                            ),
                        },
                        count: None,
                    }],
                });
        let shader = world.resource::<AssetServer>().load("shaders/simple.wgsl");
        let mut pipeline_cache = world.resource_mut::<PipelineCache>();
        let update_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: None,
            layout: Some(vec![texture_bind_group_layout.clone()]),
            shader,
            shader_defs: vec![],
            entry_point: Cow::from("update"),
        });

        SlimePipeline {
            texture_bind_group_layout,
            update_pipeline,
        }
    }
}

enum SlimeState {
    Loading,
    Update,
}

struct SlimeNode {
    state: SlimeState,
}

impl Default for SlimeNode {
    fn default() -> Self {
        Self {
            state: SlimeState::Loading,
        }
    }
}

impl render_graph::Node for SlimeNode {
    fn update(&mut self, world: &mut World) {
        let pipeline = world.resource::<SlimePipeline>();
        let pipeline_cache = world.resource::<PipelineCache>();

        // if the corresponding pipeline has loaded, transition to the next stage
        match self.state {
            SlimeState::Loading => {
                if let CachedPipelineState::Ok(_) =
                    pipeline_cache.get_compute_pipeline_state(pipeline.update_pipeline)
                {
                    self.state = SlimeState::Update;
                }
            }
            SlimeState::Update => {}
        }
    }

    fn run(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), render_graph::NodeRunError> {
        let texture_bind_group = &world.resource::<SlimeBindGroup>().0;
        let pipeline_cache = world.resource::<PipelineCache>();
        let pipeline = world.resource::<SlimePipeline>();

        let mut pass = render_context
            .command_encoder
            .begin_compute_pass(&ComputePassDescriptor::default());

        pass.set_bind_group(0, texture_bind_group, &[]);

        // select the pipeline based on the current state
        match self.state {
            SlimeState::Loading => {}
            SlimeState::Update => {
                let update_pipeline = pipeline_cache
                    .get_compute_pipeline(pipeline.update_pipeline)
                    .unwrap();
                pass.set_pipeline(update_pipeline);
                pass.dispatch_workgroups(1, 1, 1);
            }
        }

        Ok(())
    }
}
