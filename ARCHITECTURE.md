# Detailed System Architecture & Data Flow

This document deep-dives into the internal mechanics of the Alloy Ingestion Engine. The architecture is designed to handle 15k+ RPS by decoupling the ingestion "hot path" from slow I/O operations.

## 🧠 Deep-Dive Diagram

[Image of a detailed software architecture diagram showing an ingestion pipeline with load balancers, parallel workers, and distributed caches]

```mermaid
graph TD
    %% Definitions and Styling
    classDef async fill:#f9f,stroke:#333,stroke-width:2px,stroke-dasharray: 5 5;
    classDef db fill:#e1f5fe,stroke:#0277bd;
    classDef buffer fill:#fff3e0,stroke:#e65100;
    classDef critical fill:#ffebee,stroke:#c62828;

    Client[k6 Load Tester] ====>|"HIGH VELOCITY POST 15k-s"| API[Axum API Handler]

    subgraph "Producer Layer (Axum/Tokio)"
        API -- "1. Zero-Copy Parse" --> Payload(Event Struct)
        Payload -- "2. L1 Lock-Free Check" --> L1Cache[L1 Cache: DashMap]
        
        L1Cache -- Hit --> Ret202[Return 202 Accepted]
        
        L1Cache -- Miss --> TrySend{"3. TrySend MPSC"}
        TrySend -- "Channel Full" --> Ret503["Return 503 Circuit Breaker"]:::critical
        TrySend -- "Success" --> MPSC
    end

    MPSC[Bounded MPSC Channel - Cap: 100k]:::buffer

    subgraph "Consumer Layer (Parallel Worker Tasks)"
        MPSC -- "Async Recv" --> W1[Worker Loop 1..N]
        
        subgraph "Worker Internal State"
            W1 -- Accumulate --> WBuffer["Internal Vec - Cap: 2500"]
            WBuffer -- "Threshold Reached" --> Batcher[Batch Processor]
            Batcher -- "Chunk 1k" --> RedisClient[Redis Manager]
        end

        RedisClient == "4. Pipelined MGET" ==> Redis[(Redis L2 Cache)]:::db
        Redis -- "Results Vector" --> Filter[Filter Unique]
        Filter -.->|"Async SETEX"| Redis
        
        Filter -- "Unique Batch Ready" --> Spawner{"5. tokio::spawn"}
    end

    %% The critical async jump
    Spawner -.- |"Fire-and-Forget"| BgFlush[Background Flusher Task]:::async
    Spawner ====|"Immediately release worker"| W1

    subgraph "Persistence Layer"
        BgFlush == "6. HTTP Async Insert" ==> ClickHouse[(ClickHouse DB)]:::db
    end