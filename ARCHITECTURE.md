# Detailed System Architecture & Data Flow

This diagram illustrates the internal mechanics of the Alloy Ingestion Engine under high load (15k RPS). It highlights the "Double-Buffering" strategy, the multi-level caching, and the asynchronous hand-offs that prevent blocking.

## 🧠 Deep-Dive Diagram

```mermaid
graph TD
    %% Definitions and Styling
    classDef async fill:#f9f,stroke:#333,stroke-width:2px,stroke-dasharray: 5 5;
    classDef db fill:#e1f5fe,stroke:#0277bd;
    classDef buffer fill:#fff3e0,stroke:#e65100;
    classDef critical fill:#ffebee,stroke:#c62828;

    Client[k6 Load Tester] ====>|HIGH VELOCITY POST (15k/s)| API[Axum API Handler]

    subgraph "Producer Layer (Axum/Tokio)"
        API -- "1. Zero-Copy Parse" --> Payload(Event Struct)
        Payload -- "2. L1 Lock-Free Check" --> L1Cache[L1 Cache: DashMap<Key, Ts>]
        
        L1Cache -- Hit (Duplicate) --> Ret202[Return 202 Accepted]
        
        L1Cache -- Miss --> TrySend{"3. TrySend(MPSC)"}
        TrySend -- "Channel Full (Backpressure)" --> Ret503["Return 503 (Circuit Breaker)"]:::critical
        TrySend -- "Space Available" --> MPSC
    end

    MPSC[Bounded MPSC Channel\nCapacity: 100k Events]:::buffer

    subgraph "Consumer Layer (Parallel Worker Tasks)"
        MPSC -- "Async Recv (Mutex Lock)" --> W1[Worker Loop 1..N]
        
        subgraph "Worker Internal State"
            W1 -- Accumulate --> WBuffer[Internal Vec<Event>\nCap: 2500]
            WBuffer -- "Threshold Reached / Tick" --> Batcher[Batch Processor]
            Batcher -- "Chunk (1k size)" --> RedisClient[Redis Connection Manager]
        end

        RedisClient == "4. Pipelined MGET Batch" ==> Redis[(Redis L2 Cache)]:::db
        Redis -- "Results Vector" --> Filter[Filter Unique Events]
        Filter -.->|"Async SETEX (New Keys)"| Redis
        
        Filter -- "Unique Batch Ready" --> Spawner{"5. tokio::spawn\n(Double Buffering)"}
    end

    %% The critical async jump
    Spawner -.- |"Fire-and-Forget Task"| BgFlush[Background Flusher Task]:::async
    Spawner ====|"Immediately release worker"| W1

    subgraph "Persistence Layer"
        BgFlush == "6. HTTP Async InsertBatch" ==> ClickHouse[(ClickHouse DB\nasync_insert=1)]:::db
    end