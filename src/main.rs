use std::{thread, time::Duration};

use load_balancer::{run_backend, run_load_balancer};

fn main() -> Result<(), std::io::Error> {
    let backend_ports = vec![8081, 8082, 8083];

    for &port in &backend_ports {
        thread::spawn(move || {
            println!("Starting backend server on port {}", port);
            if let Err(e) = run_backend(port) {
                eprintln!("Backend server on port {} error: {}", port, e);
            }
        });
    }

    println!("Waiting for backend servers to start...");
    thread::sleep(Duration::from_secs(2));

    println!("Starting load balancer...");
    run_load_balancer(8080, backend_ports)
}
