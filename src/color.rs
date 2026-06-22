use anyhow::Result;

#[repr(C)]
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct FilterParams {
    pub brightness: f32,  // -100.0 to 100.0, mapped to -1.0 to 1.0
    pub contrast: f32,    // -100.0 to 100.0, mapped to -1.0 to 1.0
    pub saturation: f32,  // -100.0 to 100.0, mapped to -1.0 to 1.0
    pub temperature: f32, // -100.0 to 100.0, mapped to -1.0 to 1.0
    pub blur: f32,        // 0.0 to 20.0 (blur radius)
}

/// Unified uniform struct matching the WGSL `ShaderUniform` layout.
/// Must be kept in sync with shader.wgsl. Total size: 40 bytes.
///
/// WGSL std140 layout (align 8 for vec2<f32>):
///   offset 0:  brightness (f32, 4 bytes)
///   offset 4:  contrast   (f32, 4 bytes)
///   offset 8:  saturation (f32, 4 bytes)
///   offset 12: temperature(f32, 4 bytes)
///   offset 16: direction  (vec2<f32>, 8 bytes, align 8)
///   offset 24: radius     (f32, 4 bytes)
///   offset 28: width      (f32, 4 bytes)
///   offset 32: height     (f32, 4 bytes)
///   offset 36: _pad       (f32, 4 bytes) — struct size rounds up to 40 for vec2 alignment
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShaderUniform {
    // Filter params (used by fs_filter)
    brightness: f32,
    contrast: f32,
    saturation: f32,
    temperature: f32,
    // Blur params (used by fs_blur)
    direction_x: f32,
    direction_y: f32,
    radius: f32,
    width: f32,
    height: f32,
    // Padding — WGSL struct alignment = max(4, 8) = 8 → size rounds to 40
    _pad: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            blur: 0.0,
        }
    }
}

/// Named filter presets available from the status bar menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterPreset {
    /// 原图 — no adjustments
    Original,
    /// 复古 — warm tone, slightly desaturated
    Vintage,
    /// 胶片 — cool temperature, enhanced contrast, mild blur
    Film,
    /// 黑白 — full desaturation
    BlackAndWhite,
    /// 低饱和 — muted colors, subtle warmth
    Muted,
    /// 日系 — bright, warm tint, soft mood
    Japanese,
    /// 赛博朋克 — cool tone, high contrast, vivid
    Cyberpunk,
    /// 暗黑 — dim, high contrast, desaturated
    Dark,
    /// 暖阳 — very warm, bright, soft glow
    Sunshine,
    /// 森林 — green tint, natural tones
    Forest,
}

impl FilterPreset {
    /// Convert preset to concrete FilterParams values
    pub fn to_params(self) -> FilterParams {
        match self {
            FilterPreset::Original => FilterParams {
                brightness: 0.0, contrast: 0.0, saturation: 0.0,
                temperature: 0.0, blur: 0.0,
            },
            FilterPreset::Vintage => FilterParams {
                brightness: 10.0, contrast: 0.0, saturation: -40.0,
                temperature: 30.0, blur: 0.0,
            },
            FilterPreset::Film => FilterParams {
                brightness: 0.0, contrast: 15.0, saturation: 0.0,
                temperature: -20.0, blur: 1.5,
            },
            FilterPreset::BlackAndWhite => FilterParams {
                brightness: 0.0, contrast: 20.0, saturation: -100.0,
                temperature: 0.0, blur: 0.0,
            },
            FilterPreset::Muted => FilterParams {
                brightness: 0.0, contrast: 0.0, saturation: -30.0,
                temperature: 10.0, blur: 0.0,
            },
            FilterPreset::Japanese => FilterParams {
                brightness: 15.0, contrast: 5.0, saturation: -10.0,
                temperature: 25.0, blur: 0.5,
            },
            FilterPreset::Cyberpunk => FilterParams {
                brightness: -5.0, contrast: 25.0, saturation: 30.0,
                temperature: -40.0, blur: 0.0,
            },
            FilterPreset::Dark => FilterParams {
                brightness: -30.0, contrast: 30.0, saturation: -30.0,
                temperature: -5.0, blur: 0.0,
            },
            FilterPreset::Sunshine => FilterParams {
                brightness: 10.0, contrast: 5.0, saturation: 10.0,
                temperature: 40.0, blur: 0.0,
            },
            FilterPreset::Forest => FilterParams {
                brightness: -5.0, contrast: 10.0, saturation: 5.0,
                temperature: -10.0, blur: 0.5,
            },
        }
    }
}

pub struct FilterPipeline {
    gpu: Option<GpuPipeline>,
}

struct GpuPipeline {
    device: wgpu::Device,
    queue: wgpu::Queue,
    filter_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl FilterPipeline {
    pub fn new() -> Self {
        match Self::init_gpu() {
            Ok(gpu) => {
                println!("wgpu GPU Filter initialized successfully.");
                Self { gpu: Some(gpu) }
            }
            Err(e) => {
                eprintln!("Failed to initialize wgpu GPU pipeline: {:?}. Falling back to CPU filtering.", e);
                Self { gpu: None }
            }
        }
    }

    fn init_gpu() -> Result<GpuPipeline> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        })).ok_or_else(|| anyhow::anyhow!("No adapter found"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("WallFlow Filter Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))?;

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("WallFlow Shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!("shader.wgsl"))),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Diffuse Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Filter Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        // Enforce the unified ShaderUniform size (40 bytes) for validation safety
                        min_binding_size: Some(std::num::NonZeroU64::new(
                            std::mem::size_of::<ShaderUniform>() as u64,
                        ).unwrap()),
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let filter_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Filter Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_filter"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Blur Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_blur"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Ok(GpuPipeline {
            device,
            queue,
            filter_pipeline,
            blur_pipeline,
            sampler,
            bind_group_layout,
        })
    }

    /// Process the image. Uses GPU if initialized, otherwise falls back to CPU.
    pub fn process_image(&self, img: &image::RgbaImage, params: &FilterParams) -> Result<image::RgbaImage> {
        if let Some(ref gpu) = self.gpu {
            match self.process_image_gpu(gpu, img, params) {
                Ok(out) => return Ok(out),
                Err(e) => {
                    eprintln!("GPU filtering failed: {:?}. Falling back to CPU.", e);
                }
            }
        }
        Ok(self.process_image_cpu(img, params))
    }

    fn process_image_gpu(&self, gpu: &GpuPipeline, img: &image::RgbaImage, params: &FilterParams) -> Result<image::RgbaImage> {
        let (width, height) = img.dimensions();

        // 1. Create source texture
        let src_texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Source Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &src_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            img.as_raw(),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let src_view = src_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // 2. Create intermediate/target textures
        let create_target_tex = || {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Target Texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };

        let filter_tex = create_target_tex();
        let filter_view = filter_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // 3. Filter Pass — unified 40-byte uniform matching shader.wgsl ShaderUniform
        let filter_uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Filter Uniform"),
            size: std::mem::size_of::<ShaderUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let filter_data = ShaderUniform {
            brightness: params.brightness / 100.0,
            contrast: params.contrast / 100.0,
            saturation: params.saturation / 100.0,
            temperature: params.temperature / 100.0,
            // Blur fields unused by fs_filter; fill with zeros
            direction_x: 0.0,
            direction_y: 0.0,
            radius: 0.0,
            width: 0.0,
            height: 0.0,
            _pad: 0.0,
        };
        gpu.queue.write_buffer(&filter_uniform, 0, bytemuck::cast_slice(&[filter_data]));

        let filter_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Filter Bind Group"),
            layout: &gpu.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: filter_uniform.as_entire_binding(),
                },
            ],
        });

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Filter Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &filter_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(&gpu.filter_pipeline);
            rpass.set_bind_group(0, &filter_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        let mut current_tex = filter_tex;
        let current_view = filter_view;

        // 4. Blur Passes (if blur is requested)
        if params.blur > 0.0 {
            let blur_uniform_h = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blur Uniform H"),
                size: std::mem::size_of::<ShaderUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let blur_data_h = ShaderUniform {
                // Filter fields unused by fs_blur; fill with zeros
                brightness: 0.0,
                contrast: 0.0,
                saturation: 0.0,
                temperature: 0.0,
                // Horizontal blur: direction = (1, 0)
                direction_x: 1.0,
                direction_y: 0.0,
                radius: params.blur.clamp(1.0, 20.0),
                width: width as f32,
                height: height as f32,
                _pad: 0.0,
            };
            gpu.queue.write_buffer(&blur_uniform_h, 0, bytemuck::cast_slice(&[blur_data_h]));

            let temp_blur_tex = create_target_tex();
            let temp_blur_view = temp_blur_tex.create_view(&wgpu::TextureViewDescriptor::default());

            let blur_bind_group_h = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blur Bind Group H"),
                layout: &gpu.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&current_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: blur_uniform_h.as_entire_binding(),
                    },
                ],
            });

            // Horizontal Pass
            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Blur H Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &temp_blur_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rpass.set_pipeline(&gpu.blur_pipeline);
                rpass.set_bind_group(0, &blur_bind_group_h, &[]);
                rpass.draw(0..3, 0..1);
            }

            // Vertical Pass
            let blur_uniform_v = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blur Uniform V"),
                size: std::mem::size_of::<ShaderUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let blur_data_v = ShaderUniform {
                // Filter fields unused by fs_blur; fill with zeros
                brightness: 0.0,
                contrast: 0.0,
                saturation: 0.0,
                temperature: 0.0,
                // Vertical blur: direction = (0, 1)
                direction_x: 0.0,
                direction_y: 1.0,
                radius: params.blur.clamp(1.0, 20.0),
                width: width as f32,
                height: height as f32,
                _pad: 0.0,
            };
            gpu.queue.write_buffer(&blur_uniform_v, 0, bytemuck::cast_slice(&[blur_data_v]));

            let final_blur_tex = create_target_tex();
            let final_blur_view = final_blur_tex.create_view(&wgpu::TextureViewDescriptor::default());

            let blur_bind_group_v = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blur Bind Group V"),
                layout: &gpu.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&temp_blur_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: blur_uniform_v.as_entire_binding(),
                    },
                ],
            });

            {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Blur V Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &final_blur_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rpass.set_pipeline(&gpu.blur_pipeline);
                rpass.set_bind_group(0, &blur_bind_group_v, &[]);
                rpass.draw(0..3, 0..1);
            }

            current_tex = final_blur_tex;
        }

        // 5. Readback Pass
        let row_bytes = width * 4;
        let padded_row_bytes = (row_bytes + 255) & !255; // aligned to 256
        let output_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Readback Buffer"),
            size: (padded_row_bytes * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &current_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &output_buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        gpu.queue.submit(Some(encoder.finish()));

        // Map buffer to CPU
        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        gpu.device.poll(wgpu::Maintain::Wait);
        rx.recv()??;

        let data = buffer_slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);

        for row in 0..height {
            let start = (row * padded_row_bytes) as usize;
            let end = start + (width * 4) as usize;
            pixels.extend_from_slice(&data[start..end]);
        }

        drop(data);
        output_buffer.unmap();

        let out_image = image::RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("Failed to construct RgbaImage from raw pixels"))?;

        Ok(out_image)
    }

    fn process_image_cpu(&self, img: &image::RgbaImage, params: &FilterParams) -> image::RgbaImage {
        let mut out_img = img.clone();

        let b = params.brightness / 100.0;
        let c = params.contrast / 100.0 + 1.0;
        let s = params.saturation / 100.0 + 1.0;
        let t = params.temperature / 100.0;

        for pixel in out_img.pixels_mut() {
            let r_in = pixel.0[0] as f32 / 255.0;
            let g_in = pixel.0[1] as f32 / 255.0;
            let b_in = pixel.0[2] as f32 / 255.0;
            let a = pixel.0[3];

            // 1. Brightness
            let mut r = r_in + b;
            let mut g = g_in + b;
            let mut b_ch = b_in + b;

            // 2. Contrast
            r = (r - 0.5) * c + 0.5;
            g = (g - 0.5) * c + 0.5;
            b_ch = (b_ch - 0.5) * c + 0.5;

            // 3. Saturation
            let luma = 0.299 * r + 0.587 * g + 0.114 * b_ch;
            r = luma + (r - luma) * s;
            g = luma + (g - luma) * s;
            b_ch = luma + (b_ch - luma) * s;

            // 4. Color Temperature
            r = r + t * 0.1;
            g = g + t * 0.05;
            b_ch = b_ch - t * 0.1;

            pixel.0[0] = (r.clamp(0.0, 1.0) * 255.0) as u8;
            pixel.0[1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
            pixel.0[2] = (b_ch.clamp(0.0, 1.0) * 255.0) as u8;
            pixel.0[3] = a;
        }

        let blur_radius = params.blur as usize;
        if blur_radius > 0 {
            out_img = gaussian_blur(&out_img, blur_radius);
        }

        out_img
    }
}

/// Separable Gaussian blur matching the GPU shader's `fs_blur`.
/// Uses σ = radius and the same exp(-i²/(2σ²)) weighting as the WGSL path,
/// so CPU fallback produces visually identical results to GPU.
fn gaussian_blur(img: &image::RgbaImage, radius: usize) -> image::RgbaImage {
    let (width, height) = img.dimensions();
    let mut temp = img.clone();
    let mut out = img.clone();

    // Clamp radius to match GPU (1..=20) and convert to signed for loop range
    let r = radius.clamp(1, 20) as i32;
    let sigma = r as f32; // σ = radius, matching GPU shader
    // Precompute Gaussian weights for kernel [-r, r]
    let weights: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();

    // Horizontal pass
    for y in 0..height {
        for x in 0..width {
            let mut r_sum = 0.0f32;
            let mut g_sum = 0.0f32;
            let mut b_sum = 0.0f32;
            let mut a_sum = 0.0f32;
            let mut total_weight = 0.0f32;

            for (k, &weight) in weights.iter().enumerate() {
                let dx = k as i32 - r;
                let px = (x as i32 + dx).clamp(0, width as i32 - 1) as u32;
                let pixel = img.get_pixel(px, y);
                r_sum += pixel[0] as f32 * weight;
                g_sum += pixel[1] as f32 * weight;
                b_sum += pixel[2] as f32 * weight;
                a_sum += pixel[3] as f32 * weight;
                total_weight += weight;
            }

            let pixel = temp.get_pixel_mut(x, y);
            pixel[0] = (r_sum / total_weight) as u8;
            pixel[1] = (g_sum / total_weight) as u8;
            pixel[2] = (b_sum / total_weight) as u8;
            pixel[3] = (a_sum / total_weight) as u8;
        }
    }

    // Vertical pass
    for x in 0..width {
        for y in 0..height {
            let mut r_sum = 0.0f32;
            let mut g_sum = 0.0f32;
            let mut b_sum = 0.0f32;
            let mut a_sum = 0.0f32;
            let mut total_weight = 0.0f32;

            for (k, &weight) in weights.iter().enumerate() {
                let dy = k as i32 - r;
                let py = (y as i32 + dy).clamp(0, height as i32 - 1) as u32;
                let pixel = temp.get_pixel(x, py);
                r_sum += pixel[0] as f32 * weight;
                g_sum += pixel[1] as f32 * weight;
                b_sum += pixel[2] as f32 * weight;
                a_sum += pixel[3] as f32 * weight;
                total_weight += weight;
            }

            let pixel = out.get_pixel_mut(x, y);
            pixel[0] = (r_sum / total_weight) as u8;
            pixel[1] = (g_sum / total_weight) as u8;
            pixel[2] = (b_sum / total_weight) as u8;
            pixel[3] = (a_sum / total_weight) as u8;
        }
    }

    out
}
