package dev.secrettunnel.probe;

import android.app.Activity;
import android.net.ConnectivityManager;
import android.net.NetworkCapabilities;
import android.widget.TextView;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import org.json.JSONObject;

/** Numeric documentation targets exist only in the disposable Linux fixture. */
final class Ipv6Probe {
    static void run(Activity activity, TextView view) {
        JSONObject result = new JSONObject();
        boolean expected = activity.getIntent().getBooleanExtra("expect_ipv6", false);
        try {
            ConnectivityManager manager = activity.getSystemService(ConnectivityManager.class);
            android.net.Network current = manager.getActiveNetwork();
            NetworkCapabilities caps = manager.getNetworkCapabilities(current);
            android.net.LinkProperties links = manager.getLinkProperties(current);
            boolean route = links != null && links.getRoutes().stream().anyMatch(r ->
                r.isDefaultRoute() && r.getDestination().getAddress() instanceof Inet6Address);
            result.put("vpn", caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN));
            result.put("ipv6_route", route);
            for (String host : new String[]{"203.0.113.10", "2001:db8::10"}) {
                String family = host.contains(":") ? "ipv6" : "ipv4";
                InetAddress target = InetAddress.getByName(host);
                byte[] payload = "tunnel-only-fixture".getBytes(StandardCharsets.UTF_8);
                boolean tcp = false, udp = false;
                try (Socket socket = new Socket()) {
                    socket.connect(new InetSocketAddress(target, 27150), 3000);
                    socket.setSoTimeout(3000);
                    socket.getOutputStream().write(payload);
                    byte[] echo = new byte[payload.length];
                    int position = 0;
                    while (position < echo.length) {
                        int length = socket.getInputStream().read(echo, position, echo.length - position);
                        if (length < 0) break;
                        position += length;
                    }
                    tcp = position == echo.length && Arrays.equals(payload, echo);
                } catch (Exception error) { result.put(family + "_tcp_error", error.toString()); }
                try (DatagramSocket socket = new DatagramSocket()) {
                    socket.setSoTimeout(3000);
                    socket.send(new DatagramPacket(payload, payload.length, target, 27150));
                    DatagramPacket reply = new DatagramPacket(new byte[512], 512);
                    socket.receive(reply);
                    udp = reply.getAddress().equals(target) && reply.getPort() == 27150 &&
                        Arrays.equals(payload, Arrays.copyOf(reply.getData(), reply.getLength()));
                } catch (Exception error) { result.put(family + "_udp_error", error.toString()); }
                result.put(family + "_tcp", tcp); result.put(family + "_udp", udp);
            }
            result.put("passed", result.getBoolean("vpn") && route == expected &&
                result.getBoolean("ipv4_tcp") && result.getBoolean("ipv4_udp") &&
                result.getBoolean("ipv6_tcp") == expected && result.getBoolean("ipv6_udp") == expected);
        } catch (Exception error) {
            try { result.put("passed", false).put("error", error.toString()); } catch (Exception ignored) { }
        }
        try (java.io.FileOutputStream file = activity.openFileOutput("ipv6.json", Activity.MODE_PRIVATE)) {
            file.write(result.toString().getBytes(StandardCharsets.UTF_8));
        } catch (Exception ignored) { }
        activity.runOnUiThread(() -> view.setText(result.toString()));
    }
}
