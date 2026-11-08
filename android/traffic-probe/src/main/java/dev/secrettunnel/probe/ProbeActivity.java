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
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.*;

/** Separate UID, no SOCKS support: every request uses ordinary Android sockets. */
public class ProbeActivity extends Activity {
    private static byte[] dnsQuery(int id) {
        byte[] query = {0,0,1,0,0,1,0,0,0,0,0,0,7,'e','x','a','m','p','l','e',3,'c','o','m',0,0,1,0,1};
        query[0]=(byte)(id>>8); query[1]=(byte)id; return query;
    }
    private static void stress(JSONObject result) throws Exception {
        ExecutorService pool=Executors.newFixedThreadPool(24);
        List<DatagramSocket> sockets=new ArrayList<>();
        try {
            List<Callable<Boolean>> queries=new ArrayList<>();
            for(int i=0;i<24;i++) {
                DatagramSocket socket=new DatagramSocket(); socket.setSoTimeout(12000); sockets.add(socket);
                final int id=1000+i;
                queries.add(()->{
                    byte[] query=dnsQuery(id);
                    socket.send(new DatagramPacket(query,query.length,InetAddress.getByName("1.1.1.1"),53));
                    DatagramPacket response=new DatagramPacket(new byte[4096],4096); socket.receive(response);
                    byte[] data=response.getData();
                    return response.getLength()>=12&&data[0]==query[0]&&data[1]==query[1]&&(data[2]&128)!=0&&(data[3]&15)==0;
                });
            }
            int replies=0;
            for(Future<Boolean> future:pool.invokeAll(queries,20,TimeUnit.SECONDS)) {
                try {if(future.get())replies++;} catch(Exception ignored) { }
            }
            result.put("udp_parallel_replies",replies);
            // Keep all UDP sockets open while HTTPS runs, as a media app would.
            List<Callable<Long>> downloads=new ArrayList<>();
            for(int i=0;i<4;i++) downloads.add(()->{
                HttpURLConnection connection=(HttpURLConnection)new URL("https://speed.cloudflare.com/__down?bytes=2097152").openConnection();
                connection.setConnectTimeout(15000);connection.setReadTimeout(30000);
                try {
                    if(connection.getResponseCode()!=200)throw new java.io.IOException("Download HTTP status");
                    long count=0;byte[] buffer=new byte[32768];
                    try(java.io.InputStream input=connection.getInputStream()) {
                        int length;while((length=input.read(buffer))>=0){count+=length;if(count>2097152)throw new java.io.IOException("Download too large");}
                    }
                    if(count!=2097152)throw new java.io.IOException("Incomplete download: "+count);
                    return count;
                } finally {connection.disconnect();}
            });
            int complete=0;long bytes=0;
            for(Future<Long> future:pool.invokeAll(downloads,45,TimeUnit.SECONDS)) {
                try {bytes+=future.get();complete++;} catch(Exception ignored) { }
            }
            result.put("https_parallel_complete",complete);result.put("https_parallel_bytes",bytes);
            result.put("stress_passed",replies==24&&complete==4);
        } finally {for(DatagramSocket socket:sockets)socket.close();pool.shutdownNow();}
    }
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
                result.put("metered",manager.isActiveNetworkMetered());
                android.net.LinkProperties links=manager.getLinkProperties(manager.getActiveNetwork());
                result.put("ipv6_route",links!=null&&links.getRoutes().stream().anyMatch(route->route.isDefaultRoute()&&route.getDestination().getAddress() instanceof Inet6Address));
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
                if(getIntent().getBooleanExtra("stress",false)) {
                    stress(result);
                    result.put("passed",result.getBoolean("passed")&&result.optBoolean("stress_passed"));
                }
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
