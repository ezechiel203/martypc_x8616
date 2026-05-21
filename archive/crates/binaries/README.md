# Archived Binary Crates

These crates are no longer maintained or built as part of the MartyPC workspace, but are retained for reference.

- `martypc_desktop_wgpu`
- `martypc_web_player_wgpu`

### martypc_desktop_wgpu
- This was a frontend that uses raw winit and wgpu, rendering egui as a separate layer to allow for
  custom shaders.  It is currently broken due to winit 0.30 completely changing its API and wgpu
  adding lifetimes that broke my DisplayManager trait. I hate lifetime annotations.

### martypc_web_player_wgpu
- This was the old wasm build of MartyPC used to make the old web demos - it's hopelessly code-rotten now and will not 
  build.
