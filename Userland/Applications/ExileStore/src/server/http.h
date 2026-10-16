#pragma once
// A deliberately small HTTP/1.1 server for the store's local web front-end.
//
// Scope: loopback only (binds 127.0.0.1), GET and POST, one request per
// connection ("Connection: close"), bounded header/body sizes, read timeouts and a
// cap on concurrent connections. It is not a general-purpose web server and has no
// TLS; it exists so a browser or web view on the same device can show the store UI.
// Everything security-relevant about *requests* (host checks, token, origin) is
// decided in the handler (api.cpp), which is unit-tested.

#include <atomic>
#include <functional>
#include <map>
#include <string>
#include <thread>

namespace exile::server {

struct Request {
    std::string method;
    std::string target;                          // raw request target
    std::string path;                            // percent-decoded path, no query
    std::string query;                           // raw query string, no '?'
    std::map<std::string, std::string> headers;  // lowercase names
    std::string body;

    std::string header(const std::string& lowerName) const {
        auto it = headers.find(lowerName);
        return it == headers.end() ? std::string() : it->second;
    }
};

struct Response {
    int status = 200;
    std::string contentType = "text/plain; charset=utf-8";
    std::string body;
    std::map<std::string, std::string> headers;
};

using Handler = std::function<Response(const Request&)>;

struct Limits {
    size_t maxHeaderBytes = 16 * 1024;
    size_t maxBodyBytes = 64 * 1024;
    int ioTimeoutSeconds = 10;
    int maxConnections = 32;
};

// Parses a raw HTTP request head+body. Returns an empty string on success or the
// reason (used as the 400 message). Exposed for tests.
std::string parseRequest(const std::string& head, Request& out);
std::string percentDecode(const std::string& s, bool& ok);
const char* reasonPhrase(int status);

class Server {
public:
    Server(Handler handler, Limits limits = {});
    ~Server();
    Server(const Server&) = delete;
    Server& operator=(const Server&) = delete;

    // Binds 127.0.0.1:port (0 = pick a free port). False + error on failure.
    bool listen(unsigned short port, std::string& error);
    unsigned short port() const { return port_; }
    void start();   // accept loop on a background thread
    void stop();    // closes the listener and waits for connections to drain

private:
    void acceptLoop();
    void serve(int fd);

    Handler handler_;
    Limits limits_;
    int listenFd_ = -1;
    unsigned short port_ = 0;
    std::atomic<bool> running_{false};
    std::atomic<int> active_{0};
    std::thread thread_;
};

}  // namespace exile::server
