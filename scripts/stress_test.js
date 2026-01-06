import http from 'k6/http';
import { check } from 'k6';

export const options = {
    stages: [
        { duration: '30s', target: 5000 },
        { duration: '1m', target: 10000 },
        { duration: '1m', target: 15000 },
        { duration: '30s', target: 0 },
    ],
};

export default function () {
    const url = 'http://localhost:3000/ingest';

    const payload = JSON.stringify({
        event_id: "550e8400-e29b-41d4-a716-446655440000",
        customer_id: "cust_8821",
        event_type: "api_call",
        amount: 1,
        idempotency_key: `unique-uuid-val-${Math.floor(Math.random() * 10000000)}`,
        timestamp: 0,
    });

    const params = {
        headers: { 'Content-Type': 'application/json' },
    };

    const res = http.post(url, payload, params);

    check(res, {
        'status is 202': (r) => r.status === 202,
    });

    //"C:\Program Files\k6\k6.exe" run stress_test.js
}