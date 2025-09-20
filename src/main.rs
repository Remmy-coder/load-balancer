use load_balancer::{init_logger, run_backend, run_load_balancer};
use std::{thread, time::Duration};

fn main() -> Result<(), std::io::Error> {
    init_logger();

    let backend_ports = vec![8081, 8082, 8083];

    for &port in &backend_ports {
        thread::spawn(move || {
            if let Err(e) = run_backend(port) {
                eprintln!("Backend {} error: {}", port, e);
            }
        });
    }

    println!("Waiting for backends...");
    thread::sleep(Duration::from_secs(1));

    run_load_balancer(8080, backend_ports, load_balancer::Strategy::RoundRobin)
}
