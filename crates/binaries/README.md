
# MartyPC Frontend Crates

Here are various frontends for MartyPC.

### martypc_eframe
 - This is a frontend build on top of [eframe](https://github.com/emilk/egui/tree/master/crates/eframe). 
   It is up to date with winit and wgpu dependencies, but currently lacks some of the features of the
   old marty_desktop_wgpu frontend, namely shaders. This is probably what you should build.

   This frontend can be built for the web using the `wasm-unknown-unknown` target.
   You can build and serve the web version locally using `trunk serve`.

### martypc_headless
 - This is a headless, cli-only frontend for MartyPC. This is used to generate or validate CPU tests and
   perform benchmarks.
