# Stage 1: build
FROM rust:1.85 as builder
WORKDIR /usr/src/app
COPY . .
RUN apt-get update && apt-get install -y pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
RUN cargo build --release

# Stage 2: runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/app/target/release/CoordinatorServer /usr/local/bin/CoordinatorServer
EXPOSE 9000/udp
ENTRYPOINT ["/usr/local/bin/CoordinatorServer"]