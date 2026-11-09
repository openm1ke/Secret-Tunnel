package dev.secrettunnel.probe;

import java.io.IOException;
import java.net.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.BooleanSupplier;
import org.json.JSONObject;

/** Same bounded HTTPS/DNS workload as scripts/benchmark-socks.py. */
final class NetworkBenchmark {
    private static final int BYTES = 4 * 1024 * 1024;
    private static final int SAMPLES = 48;

    private static double percentile(List<Double> sorted, double p) {
        return sorted.get(Math.max(0, (int)Math.ceil(p * sorted.size()) - 1));
    }

    private static JSONObject udp(BooleanSupplier continueSending) throws Exception {
        List<Double> rtts = new ArrayList<>();
        int sent = 0;
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(InetAddress.getByName("1.1.1.1"), 53);
            socket.setSoTimeout(1500);
            byte[] query = {0,0,1,0,0,1,0,0,0,0,0,0,7,'e','x','a','m','p','l','e',3,'c','o','m',0,0,1,0,1};
            byte[] data = new byte[4096];
            for (int i = 0; i < SAMPLES && continueSending.getAsBoolean(); i++) {
                int id = 0x4100 + i;
                query[0] = (byte)(id >> 8); query[1] = (byte)id;
                long start = System.nanoTime();
                socket.send(new DatagramPacket(query, query.length));
                sent++;
                try {
                    DatagramPacket packet = new DatagramPacket(data, data.length);
                    socket.receive(packet);
                    if (packet.getLength() >= 12 && data[0] == query[0] && data[1] == query[1]
                            && (data[2] & 128) != 0 && (data[3] & 15) == 0) {
                        rtts.add((System.nanoTime() - start) / 1e6);
                    }
                } catch (SocketTimeoutException ignored) { }
                Thread.sleep(20);
            }
        }
        JSONObject result = new JSONObject().put("sent", sent).put("received", rtts.size());
        if (sent > 0) result.put("loss_percent", 100.0 * (sent - rtts.size()) / sent);
        if (!rtts.isEmpty()) {
            double variation = 0;
            for (int i = 1; i < rtts.size(); i++) variation += Math.abs(rtts.get(i) - rtts.get(i-1));
            if (rtts.size() > 1) result.put("mean_rtt_delta_ms", variation / (rtts.size() - 1));
            Collections.sort(rtts);
            result.put("p50_ms", percentile(rtts, .50)).put("p95_ms", percentile(rtts, .95))
                .put("p99_ms", percentile(rtts, .99));
        }
        return result;
    }

    private static long download() throws Exception {
        HttpURLConnection connection = (HttpURLConnection)new URL("https://speed.cloudflare.com/__down?bytes=" + BYTES).openConnection();
        connection.setConnectTimeout(12000); connection.setReadTimeout(15000);
        try {
            if (connection.getResponseCode() != 200) throw new IOException("Download HTTP status");
            long received = 0;
            byte[] buffer = new byte[32768];
            try (java.io.InputStream input = connection.getInputStream()) {
                for (int count; (count = input.read(buffer)) >= 0;) {
                    received += count;
                    if (received > BYTES) throw new IOException("Oversized download");
                }
            }
            if (received != BYTES) throw new IOException("Incomplete download");
            return received;
        } finally { connection.disconnect(); }
    }

    static void run(JSONObject result) throws Exception {
        result.put("udp_idle", udp(() -> true));
        ExecutorService workers = Executors.newFixedThreadPool(4);
        AtomicInteger remaining = new AtomicInteger(4);
        AtomicLong finished = new AtomicLong();
        List<Future<Long>> downloads = new ArrayList<>();
        long start = System.nanoTime();
        try {
            for (int i = 0; i < 4; i++) downloads.add(workers.submit(() -> {
                try { return download(); }
                finally {
                    finished.accumulateAndGet(System.nanoTime(), Math::max);
                    remaining.decrementAndGet();
                }
            }));
            result.put("udp_bulk", udp(() -> remaining.get() > 0));
            int complete = 0;
            long bytes = 0;
            for (Future<Long> future : downloads) {
                try { bytes += future.get(35, TimeUnit.SECONDS); complete++; }
                catch (ExecutionException | TimeoutException ignored) { }
            }
            double seconds = (finished.get() - start) / 1e9;
            result.put("tcp_complete", complete).put("tcp_bytes", bytes);
            if (seconds > 0) result.put("tcp_seconds", seconds).put("tcp_mbps", bytes * 8 / seconds / 1e6);
            result.put("benchmark_passed", complete == 4 && result.getJSONObject("udp_idle").getInt("received") > 0);
        } finally { workers.shutdownNow(); }
    }
}
