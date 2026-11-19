namespace net {

class Client {
public:
    void connect(int port);
    int port() const { return port_; }
private:
    int port_;
};

void Client::connect(int port) {
    port_ = port;
}

}  // namespace net

int main() {
    return 0;
}
