// Copyright 2014 The Gfx-rs Developers.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use device;
use device::shade::{ProgramMeta, Vertex, Fragment, UniformValue};
use self::envir::BindableStorage;
pub use BufferHandle = device::dev::Buffer;

pub type MeshHandle = uint;
pub type SurfaceHandle = device::dev::Surface;
pub type TextureHandle = device::dev::Texture;
pub type SamplerHandle = uint;
pub type ProgramHandle = uint;
pub type EnvirHandle = uint;

pub mod envir;
pub mod mesh;
pub mod target;

/// Temporary cache system before we get the handle manager
struct Cache {
    pub meshes: Vec<mesh::Mesh>,
    pub programs: Vec<ProgramMeta>,
    pub environments: Vec<envir::Storage>,
}

pub struct Renderer {
    device: device::Client,
    /// a common VAO for mesh rendering
    common_array_buffer: Option<device::dev::ArrayBuffer>,
    /// the default FBO for drawing
    default_frame_buffer: device::dev::FrameBuffer,
    /// cached meta-data for meshes and programs
    cache: Cache,
}

impl Renderer {
    pub fn new(device: device::Client) -> Renderer {
        Renderer {
            device: device,
            common_array_buffer: None,
            default_frame_buffer: 0,
            cache: Cache {
                meshes: Vec::new(),
                programs: Vec::new(),
                environments: Vec::new(),
            },
        }
    }

    fn get_common_array_buffer(&mut self) -> device::dev::ArrayBuffer {
        match self.common_array_buffer {
            Some(array_buffer) => array_buffer,
            None => {
                self.device.send(device::CallNewArrayBuffer);
                match self.device.recv() {
                    device::ReplyNewArrayBuffer(array_buffer) => {
                        self.common_array_buffer = Some(array_buffer);
                        array_buffer
                    },
                    _ => fail!("invalid device reply for CallNewArrayBuffer"),
                }
            },
        }
    }

    pub fn clear(&mut self, data: target::ClearData, frame: Option<target::Frame>) {
        self.bind_frame(&frame);
        match data.color {
            Some(col) => self.device.send(device::CastClear(col)),
            None => unimplemented!(),
        }
    }

    pub fn draw(&mut self, mesh_handle: MeshHandle, slice: mesh::Slice, frame: Option<target::Frame>, program_handle: ProgramHandle, env_handle: EnvirHandle) {
        // bind output frame
        self.bind_frame(&frame);
        // get array buffer for later
        let array_buffer = self.get_common_array_buffer();
        // bind shaders
        let program = self.cache.programs.get(program_handle);
        let env = self.cache.environments.get(env_handle);
        match env.optimize(program) {
            Ok(ref cut) => Renderer::bind_environment(&mut self.device, env, cut, program),
            Err(err) => {
                error!("Failed to build environment shortcut {}", err);
                return;
            },
        }
        // bind vertex attributes
        self.device.send(device::CastBindArrayBuffer(array_buffer));
        let mesh = self.cache.meshes.get(mesh_handle);
        Renderer::bind_mesh(&mut self.device, mesh, program).unwrap();
        // draw
        match slice {
            mesh::VertexSlice(start, end) => {
                self.device.send(device::CastDraw(start, end));
            },
            mesh::IndexSlice(buf, start, end) => {
                self.device.send(device::CastBindIndex(buf));
                self.device.send(device::CastDrawIndexed(start, end));
            },
        }
    }

    pub fn end_frame(&self) {
        self.device.send(device::CastSwapBuffers);
    }

    pub fn create_program(&mut self, vs_src: Vec<u8>, fs_src: Vec<u8>) -> ProgramHandle {
        self.device.send(device::CallNewShader(Vertex, vs_src));
        self.device.send(device::CallNewShader(Fragment, fs_src));
        let h_vs = match self.device.recv() {
            device::ReplyNewShader(name) => name.unwrap_or(0),
            msg => fail!("invalid device reply for CallNewShader: {}", msg)
        };
        let h_fs = match self.device.recv() {
            device::ReplyNewShader(name) => name.unwrap_or(0),
            msg => fail!("invalid device reply for CallNewShader: {}", msg)
        };
        self.device.send(device::CallNewProgram(vec![h_vs, h_fs]));
        match self.device.recv() {
            device::ReplyNewProgram(Ok(prog)) => {
                self.cache.programs.push(prog);
                self.cache.programs.len() - 1
            },
            device::ReplyNewProgram(Err(_)) => 0,
            _ => fail!("invalid device reply for CallNewProgram"),
        }
    }

    pub fn create_mesh(&mut self, num_vert: mesh::VertexCount, data: Vec<f32>, count: u8, stride: u8) -> MeshHandle {
        self.device.send(device::CallNewVertexBuffer(data));
        let buffer = match self.device.recv() {
            device::ReplyNewBuffer(name) => name,
            _ => fail!("invalid device reply for CallNewVertexBuffer")
        };
        let mut mesh = mesh::Mesh::new(num_vert);
        mesh.attributes.push(mesh::Attribute {
            buffer: buffer,
            size: count,
            offset: 0,
            stride: stride,
            is_normalized: false,
            is_interpolated: false,
            name: "a_Pos".to_string(),
        });
        let handle = self.cache.meshes.len();
        self.cache.meshes.push(mesh);
        handle
    }

    pub fn create_index_buffer(&self, data: Vec<u16>) -> BufferHandle {
        self.device.send(device::CallNewIndexBuffer(data));
        match self.device.recv() {
            device::ReplyNewBuffer(name) => name,
            _ => fail!("invalid device reply for CallNewIndexBuffer"),
        }
    }

    pub fn create_raw_buffer(&self) -> BufferHandle {
        self.device.send(device::CallNewRawBuffer);
        match self.device.recv() {
            device::ReplyNewBuffer(name) => name,
            _ => fail!("invalid device reply for CallNewRawBuffer"),
        }
    }

    pub fn create_environment(&mut self, storage: envir::Storage) -> EnvirHandle {
        let handle = self.cache.environments.len();
        self.cache.environments.push(storage);
        handle
    }

    pub fn set_env_block(&mut self, handle: EnvirHandle, var: envir::BlockVar, buf: BufferHandle) {
        self.cache.environments.get_mut(handle).set_block(var, buf);
    }

    pub fn set_env_uniform(&mut self, handle: EnvirHandle, var: envir::UniformVar, value: UniformValue) {
        self.cache.environments.get_mut(handle).set_uniform(var, value);
    }

    pub fn set_env_texture(&mut self, handle: EnvirHandle, var: envir::TextureVar, texture: TextureHandle, sampler: SamplerHandle) {
        self.cache.environments.get_mut(handle).set_texture(var, texture, sampler);
    }

    pub fn update_buffer(&self, buf: BufferHandle, data: Vec<f32>) {
        self.device.send(device::CastUpdateBuffer(buf, data));
    }

    fn bind_frame(&mut self, frame_opt: &Option<target::Frame>) {
        match frame_opt {
            &Some(ref _frame) => {
                //TODO: find an existing FBO that matches the plane set
                // or create a new one and bind it
                unimplemented!()
            },
            &None => {
                self.device.send(device::CastBindFrameBuffer(self.default_frame_buffer));
            }
        }
    }

    fn bind_mesh(device: &mut device::Client, mesh: &mesh::Mesh, prog: &ProgramMeta) -> Result<(),()> {
        for sat in prog.attributes.iter() {
            match mesh.attributes.iter().find(|a| a.name.as_slice() == sat.name.as_slice()) {
                Some(vat) => device.send(device::CastBindAttribute(sat.location as u8,
                    vat.buffer, vat.size as u32, vat.offset as u32, vat.stride as u32)),
                None => return Err(())
            }
        }
        Ok(())
    }

    fn bind_environment(device: &mut device::Client, storage: &envir::Storage, shortcut: &envir::Shortcut, program: &ProgramMeta) {
        debug_assert!(storage.is_fit(shortcut, program));
        device.send(device::CastBindProgram(program.name));

        for (i, (&k, block_var)) in shortcut.blocks.iter().zip(program.blocks.iter()).enumerate() {
            let block = storage.get_block(k);
            block_var.active_slot.set(i as u8);
            device.send(device::CastBindUniformBlock(program.name, i as u8, i as device::UniformBufferSlot, block));
        }

        for (&k, uniform_var) in shortcut.uniforms.iter().zip(program.uniforms.iter()) {
            let value = storage.get_uniform(k);
            uniform_var.active_value.set(value);
            device.send(device::CastBindUniform(uniform_var.location, value));
        }

        for (_i, (&_k, _texture)) in shortcut.textures.iter().zip(program.textures.iter()).enumerate() {
            unimplemented!()
        }
    }
}
