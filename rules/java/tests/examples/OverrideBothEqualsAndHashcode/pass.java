final class Point {
    final int x;

    Point(int x) {
        this.x = x;
    }

    @Override
    public boolean equals(Object other) {
        return other instanceof Point && ((Point) other).x == x;
    }

    @Override
    public int hashCode() {
        return x;
    }
}
