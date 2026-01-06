import http from 'k6/http';
import { check } from 'k6';

export const options = {
    scenarios: {
        constant_request_rate: {
            executor: 'constant-arrival-rate',
            rate: 15000,
            timeUnit: '1s',
            duration: '3m',
            preAllocatedVUs: 1000,
            maxVUs: 2000,
        },
    },
};

export default function () {
    const url = 'http://localhost:3000/ingest';

    const payload = JSON.stringify({
        event_id: "550e8400-e29b-41d4-a716-446655440000",
        customer_id: "cust_8821",
        event_type: "api_call",
        amount: 1,
        idempotency_key: `key-${Math.floor(Math.random() * 10000000)}`,
        timestamp: 1,
    });

    const params = {
        headers: { 'Content-Type': 'application/json' },
    };

    const res = http.post(url, payload, params);

    check(res, {
        'status is 202': (r) => r.status === 202,
        'body has success message': (r) => r.body && r.body.includes('accepted'),
    });
}