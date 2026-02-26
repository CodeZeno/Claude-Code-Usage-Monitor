#![windows_subsystem = "windows"]

mod models;
mod native_interop;
mod poller;
mod theme;
mod window;

fn main() {
    window::run();
}
