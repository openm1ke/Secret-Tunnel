package dev.secrettunnel.probe;

import android.app.Activity;
import android.os.SystemClock;
import android.widget.TextView;
import org.json.JSONObject;
import java.io.*;
import java.net.*;
import java.util.*;
import java.util.concurrent.*;

/** Opt-in, numeric loopback-fixture workload; no public download services. */
final class LongSessionProbe {
    static void run(Activity activity, TextView view) {
        long start = SystemClock.elapsedRealtime();
        long seconds = Math.max(10, Math.min(7200, activity.getIntent().getIntExtra("soak_seconds", 60)));
        String host = activity.getIntent().getStringExtra("soak_host");
        int port = activity.getIntent().getIntExtra("soak_port", 27150);
        int readTimeout = Math.max(2000, Math.min(60000,
            activity.getIntent().getIntExtra("soak_tcp_timeout_ms", 8000)));
        int failureBudget = Math.max(0, Math.min(1000,
            activity.getIntent().getIntExtra("soak_failure_budget", 10)));
        ExecutorService workers = Executors.newFixedThreadPool(8);
        List<DatagramSocket> udp = new ArrayList<>();
        long tcpOk = 0, tcpAborted = 0, tcpErrors = 0, udpOk = 0, udpErrors = 0;
        int cycle = 0;
        String lastError = "";
        try {
            // The documentation address is bound only inside the disposable VM.
            InetAddress target = InetAddress.getByName(host == null ? "203.0.113.10" : host);
            for (int i = 0; i < 8; i++) {
                DatagramSocket socket = new DatagramSocket();
                socket.setSoTimeout(3000);
                udp.add(socket);
            }
            while (SystemClock.elapsedRealtime() - start < seconds * 1000) {
                final int sequence = cycle++;
                List<Callable<Boolean>> batch = new ArrayList<>();
                for (int i = 0; i < 8; i++) {
                    final boolean abort = i == 7;
                    batch.add(() -> {
                        try (Socket socket = new Socket()) {
                            socket.setTcpNoDelay(true);
                            socket.setSoTimeout(readTimeout);
                            socket.connect(new InetSocketAddress(target, port), 8000);
                            byte[] payload = new byte[abort ? 8192 : 131072];
                            Arrays.fill(payload, (byte) sequence);
                            socket.getOutputStream().write(payload);
                            if (abort) { socket.setSoLinger(true, 0); return false; }
                            socket.shutdownOutput();
                            byte[] received = new byte[16384];
                            long bytes = 0;
                            for (int n; (n = socket.getInputStream().read(received)) >= 0;) {
                                for (int j = 0; j < n; j++) {
                                    if (received[j] != (byte) sequence) throw new IOException("TCP payload mismatch");
                                }
                                bytes += n;
                            }
                            if (bytes != payload.length) throw new IOException("Truncated TCP response: " + bytes);
                            return true;
                        }
                    });
                }
                List<Future<Boolean>> running = new ArrayList<>();
                for (Callable<Boolean> task : batch) running.add(workers.submit(task));
                for (int i = 0; i < udp.size(); i++) {
                    byte[] payload = new byte[1200];
                    Arrays.fill(payload, (byte) sequence);
                    payload[0] = (byte) (sequence >>> 24); payload[1] = (byte) (sequence >>> 16);
                    payload[2] = (byte) (sequence >>> 8); payload[3] = (byte) sequence;
                    DatagramSocket socket = udp.get(i);
                    try {
                        socket.send(new DatagramPacket(payload, payload.length, target, port));
                        DatagramPacket reply = new DatagramPacket(new byte[1500], 1500);
                        long deadline = SystemClock.elapsedRealtime() + 3000;
                        do {
                            socket.setSoTimeout((int) Math.max(1, deadline - SystemClock.elapsedRealtime()));
                            reply.setLength(reply.getData().length);
                            socket.receive(reply);
                            // Loss/delay injection may deliver a previous round
                            // after its timeout. Wait for this round's reply.
                        } while (reply.getLength() >= 4 &&
                            !Arrays.equals(Arrays.copyOf(reply.getData(), 4), Arrays.copyOf(payload, 4)) &&
                            SystemClock.elapsedRealtime() < deadline);
                        if (!Arrays.equals(payload, Arrays.copyOf(reply.getData(), reply.getLength())))
                            throw new IOException("UDP payload mismatch");
                        udpOk++;
                    } catch (IOException error) { udpErrors++; lastError = error.toString(); }
                }
                for (Future<Boolean> task : running) {
                    try { if (task.get(readTimeout + 5000L, TimeUnit.MILLISECONDS)) tcpOk++; else tcpAborted++; }
                    catch (Exception error) { tcpErrors++; lastError = error.toString(); }
                }
                JSONObject sample = new JSONObject().put("elapsed_ms", SystemClock.elapsedRealtime() - start)
                    .put("cycles", cycle).put("tcp_ok", tcpOk).put("tcp_aborted", tcpAborted)
                    .put("tcp_errors", tcpErrors).put("udp_ok", udpOk).put("udp_errors", udpErrors)
                    .put("last_error", lastError).put("finished", false);
                write(activity, sample);
                final String text = sample.toString(2);
                activity.runOnUiThread(() -> view.setText(text));
                if (tcpErrors > failureBudget || udpErrors > failureBudget) throw new IOException("Repeated traffic failures");
                Thread.sleep(250);
            }
            write(activity, new JSONObject().put("elapsed_ms", SystemClock.elapsedRealtime() - start)
                .put("cycles", cycle).put("tcp_ok", tcpOk).put("tcp_aborted", tcpAborted)
                .put("tcp_errors", tcpErrors).put("udp_ok", udpOk).put("udp_errors", udpErrors)
                .put("finished", true).put("passed", tcpErrors == 0 && udpErrors == 0));
        } catch (Exception error) {
            try { write(activity, new JSONObject().put("finished", true).put("passed", false)
                .put("elapsed_ms", SystemClock.elapsedRealtime() - start).put("error", error.toString())
                .put("cycles", cycle).put("tcp_ok", tcpOk).put("tcp_errors", tcpErrors)
                .put("udp_ok", udpOk).put("udp_errors", udpErrors)); } catch (Exception ignored) { }
        } finally {
            for (DatagramSocket socket : udp) socket.close();
            workers.shutdownNow();
        }
    }

    private static void write(Activity activity, JSONObject value) throws IOException {
        try (FileOutputStream file = activity.openFileOutput("soak.json", Activity.MODE_PRIVATE)) {
            file.write(value.toString().getBytes(java.nio.charset.StandardCharsets.UTF_8));
        }
    }
}
