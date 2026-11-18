package demo;

/** Docs. */
public class Sample implements Runnable {
    public Sample(int value) {
    }

    @Override
    public void run() {
    }

    interface Listener {
        void onEvent(String name);
    }
}

enum Color { RED, GREEN }
