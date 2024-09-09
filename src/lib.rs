use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

pub struct LoadBalancer {
    backends: Vec<String>,
    current: usize,
}

impl LoadBalancer {
    pub fn new(backends: Vec<String>) -> Self {
        LoadBalancer {
            backends,
            current: 0,
        }
    }

    pub fn next_backend(&mut self) -> &str {
        let backend = &self.backends[self.current];
        self.current = (self.current + 1) % self.backends.len();
        backend
    }
}

pub fn handle_client(mut client: TcpStream, backend: &str) -> Result<(), std::io::Error> {
    println!(
        "Handling client request, forwarding to backend: {}",
        backend
    );
    let mut server = TcpStream::connect(backend)?;
    println!("Connected to backend server");

    client.set_read_timeout(Some(Duration::from_secs(5)))?;
    server.set_read_timeout(Some(Duration::from_secs(5)))?;

    let mut buffer = [0; 1024];

    // Read the request from the client and forward it to the backend
    loop {
        match client.read(&mut buffer) {
            Ok(0) => {
                println!("Client closed the connection before sending data");
                break;
            }
            Ok(n) => {
                println!("Read {} bytes from client", n);
                if let Err(e) = server.write_all(&buffer[..n]) {
                    println!("Error writing to backend server: {}", e);
                    return Err(e);
                }
                server.flush()?;
                println!("Wrote {} bytes to backend server", n);
            }
            Err(e) => {
                println!("Error reading from client: {}", e);
                return Err(e);
            }
        }

        // Read the response from the backend and send it to the client
        match server.read(&mut buffer) {
            Ok(0) => {
                println!("Backend closed the connection without sending data");
                break;
            }
            Ok(n) => {
                println!("Read {} bytes from backend", n);
                if let Err(e) = client.write_all(&buffer[..n]) {
                    println!("Error writing to client: {}", e);
                    return Err(e);
                }
                client.flush()?;
                println!("Wrote {} bytes back to client", n);
            }
            Err(e) => {
                println!("Error reading from backend: {}", e);
                return Err(e);
            }
        }
    }

    Ok(())
}

pub fn run_backend(port: u16) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    println!("Backend server listening on 127.0.0.1:{}", port);

    for stream in listener.incoming() {
        let mut stream = stream?;
        println!("Backend on port {} received a connection", port);

        // Send a valid HTTP response with headers and body
        let body = format!("Response from backend on port {}\n", port);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{}",
            body.len(),
            body
        );

        match stream.write_all(response.as_bytes()) {
            Ok(_) => {
                stream.flush()?;
                println!("Backend on port {} sent response", port);
            }
            Err(e) => println!("Backend on port {} error sending response: {}", port, e),
        }

        // Ensure the response is sent before closing the connection
        stream.flush()?;
    }
    Ok(())
}

pub fn run_load_balancer(port: u16, backend_ports: Vec<u16>) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
    let mut load_balancer = LoadBalancer::new(
        backend_ports
            .iter()
            .map(|p| format!("127.0.0.1:{}", p))
            .collect(),
    );

    println!("Load balancer listening on 127.0.0.1:{}", port);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let backend = load_balancer.next_backend().to_string();
                println!("New connection, forwarding to {}", backend);
                let backend_clone = backend.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream, &backend_clone) {
                        eprintln!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                eprintln!("Error accepting connection: {}", e);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_load_balancer_next_backend() {
        let backends = vec![
            "127.0.0.1:8081".to_string(),
            "127.0.0.1:8082".to_string(),
            "127.0.0.1:8083".to_string(),
        ];
        let mut lb = LoadBalancer::new(backends);

        assert_eq!(lb.next_backend(), "127.0.0.1:8081");
        assert_eq!(lb.next_backend(), "127.0.0.1:8082");
        assert_eq!(lb.next_backend(), "127.0.0.1:8083");
        assert_eq!(lb.next_backend(), "127.0.0.1:8081"); // Should wrap around
    }

    #[test]
    fn test_run_backend() {
        let port = 8084;
        thread::spawn(move || {
            run_backend(port).unwrap();
        });

        thread::sleep(Duration::from_millis(100)); // Give the backend time to start

        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();

        assert!(response.contains(&format!("Response from backend on port {}", port)));
    }
}
