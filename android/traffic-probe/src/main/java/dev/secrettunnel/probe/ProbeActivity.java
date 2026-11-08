package dev.secrettunnel.probe;

import android.app.Activity;
import android.net.ConnectivityManager;
import android.net.NetworkCapabilities;
import android.os.Bundle;
import android.widget.TextView;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import org.json.JSONObject;

/** Separate UID, no SOCKS support: every request uses ordinary Android sockets. */
public class ProbeActivity extends Activity {
    private static byte[] read(java.io.InputStream input) throws java.io.IOException {
        try (java.io.InputStream stream = input; java.io.ByteArrayOutputStream output = new java.io.ByteArrayOutputStream()) {
            byte[] buffer = new byte[8192]; int length;
            while ((length = stream.read(buffer)) >= 0) {
                if (output.size() + length > 1024 * 1024) throw new java.io.IOException("Response too large");
                output.write(buffer, 0, length);
            }
            return output.toByteArray();
        }
    }
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        TextView output = new TextView(this); output.setPadding(24, 48, 24, 24);
        output.setText("Checking raw TCP, UDP and DNS…"); setContentView(output);
        new Thread(() -> {
            JSONObject result = new JSONObject();
            try {
                result.put("package", getPackageName());
                ConnectivityManager manager = getSystemService(ConnectivityManager.class);
                NetworkCapabilities caps = manager.getNetworkCapabilities(manager.getActiveNetwork());
                result.put("vpn", caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN));
                result.put("dns", InetAddress.getAllByName("example.com").length > 0);
                HttpURLConnection https = (HttpURLConnection)new URL("https://example.com/").openConnection();
                https.setConnectTimeout(12000); https.setReadTimeout(12000);
                try { result.put("https_status", https.getResponseCode());
                    result.put("https_bytes", read(https.getInputStream()).length);
                } finally { https.disconnect(); }
                HttpURLConnection ip = (HttpURLConnection)new URL("https://api.ipify.org/").openConnection();
                ip.setConnectTimeout(12000); ip.setReadTimeout(12000);
                try { result.put("egress_ip", new String(read(ip.getInputStream()), StandardCharsets.UTF_8).trim()); }
                finally { ip.disconnect(); }
                int replies = 0;
                try (DatagramSocket udp = new DatagramSocket()) {
                    udp.setSoTimeout(8000);
                    byte[] query = {0,0,1,0,0,1,0,0,0,0,0,0,7,'e','x','a','m','p','l','e',3,'c','o','m',0,0,1,0,1};
                    for (int i = 0; i < 8; i++) {
                        int id = new SecureRandom().nextInt(65536);
                        query[0] = (byte)(id >> 8); query[1] = (byte)id;
                        udp.send(new DatagramPacket(query, query.length, InetAddress.getByName("1.1.1.1"), 53));
                        DatagramPacket response = new DatagramPacket(new byte[4096], 4096); udp.receive(response);
                        byte[] data = response.getData();
                        if (response.getLength() < 12 || data[0] != query[0] || data[1] != query[1] || (data[2] & 128) == 0 || (data[3] & 15) != 0)
                            throw new IllegalStateException("Invalid UDP DNS response");
                        replies++;
                    }
                }
                result.put("udp_dns_replies", replies);
                result.put("passed", result.getBoolean("dns") && result.getInt("https_status") == 200 && replies == 8);
            } catch (Exception error) {
                try { result.put("passed", false); result.put("error", error.toString()); } catch (Exception ignored) { }
            }
            try (java.io.FileOutputStream file = openFileOutput("result.json", MODE_PRIVATE)) {
                file.write(result.toString(2).getBytes(StandardCharsets.UTF_8));
            } catch (Exception ignored) { }
            runOnUiThread(() -> output.setText(result.optBoolean("passed") ?
                "PASS\nVPN: " + result.optBoolean("vpn") + "\nHTTPS 200\nDNS OK\nUDP DNS: 8/8\n" + getPackageName() :
                "FAIL\n" + result.optString("error")));
        }, "raw-network-probe").start();
    }
}
