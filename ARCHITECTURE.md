# Alloy System Architecture

This document outlines the technical architecture of the Alloy Ingestion Engine, detailing how data flows from a client request to permanent storage in ClickHouse.

## High-Level Data Flow

The system follows a **Producer-Consumer** pattern decoupled by a bounded MPSC (Multi-Producer, Multi-Consumer) channel to ensure high availability and backpressure management.



```mermaid
graph TD
    Client[k6 / Client] -->|HTTP POST| Axum[Axum API Handler]
    
    subgraph "Ingestion API (Producer)"
        Axum --> L1[L1 Cache: DashMap]
        L1 -->|Unique| MPSC[Bounded MPSC Channel]
        L1 -->|Duplicate| Resp202[Return 202 Accepted]
    end

    MPSC -->|Full| Backpressure[Return 503 Service Unavailable]

    subgraph "Event Workers (Consumers)"
        MPSC --> Worker1[Worker Task 1]
        MPSC --> Worker2[Worker Task 2]
        Worker1 --> L2[L2 Cache: Redis MGET]
        Worker2 --> L2
    end

    subgraph "Persistence Layer"
        L2 -->|Unique Batch| CH[ClickHouse: async_insert]
        L2 -->|Duplicate| Sink[Drop Event]
    end