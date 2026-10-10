import os


def mkdir_p(path):
    os.makedirs(path, exist_ok=True)
