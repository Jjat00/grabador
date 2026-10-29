#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // La app se relanza a sí misma como proceso de grabación en segundo plano.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(grabador_core::WORKER_ARG) {
        return grabador_core::run_worker(&args.next().unwrap_or_default());
    }
    grabador_app_lib::run();
}
