package main

import (
	"archive/zip"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"github.com/aws/aws-sdk-go-v2/config"

	"github.com/containerd/containerd/namespaces"

	"github.com/gorilla/mux"

	"github.com/Eventual-Inc/Daft/pkg/container_runtime"

	"github.com/Eventual-Inc/Daft/pkg/objectstorage"
)

const ContainerHostPath = "/run/eventual/container/"
const TestImagesZipS3Path = "s3://eventual-data-test-bucket/test-rickroll/rickroll-images.zip"
const ContainerdSocket = "/run/containerd/containerd.sock"

// Code that will launch a reader container using the host's containerd client
func launchReader(id int, localImagesPath string) {
	// Create a containerd client
	ctx := namespaces.WithNamespace(context.Background(), "reader")

	ImageURL := "941892620273.dkr.ecr.us-west-2.amazonaws.com/daft/reader:0"

	thing := container_runtime.NewContainerRuntimeContext(ContainerdSocket, ContainerHostPath)
	thing.PullImage(ctx, ImageURL)
	containerName, _ := thing.CreateContainer(ctx, ImageURL)
	thing.StartContainer(ctx, containerName)
	thing.StopContainer(ctx, containerName)
	thing.DeleteContainer(ctx, containerName)
	thing.EvictImage(ctx, ImageURL)
	return

	// Sleep for a while to let container start UDS server
	// time.Sleep(1 * time.Second)
	// start = time.Now()

	// // Send some data to the running task
	// c, err := net.Dial("unix", sockAddr)
	// if err != nil {
	// 	log.Fatal(err)
	// }
	// defer c.Close()
	// log.Printf("Time to dial UDS: %v", time.Since(start))

	// files, err := ioutil.ReadDir(localImagesPath)
	// if err != nil {
	// 	log.Fatal(err)
	// }
	// for i, f := range files {
	// 	data, err := os.ReadFile(filepath.Join(localImagesPath, f.Name()))
	// 	if err != nil {
	// 		log.Fatal(err)
	// 	}

	// 	log.Printf("Hash of file: %x", md5.Sum(data))

	// 	start = time.Now()
	// 	builder := flatbuffers.NewBuilder(len(data)) // figure out better initial sizing
	// 	filedata := builder.CreateByteVector(data)
	// 	fbs.FileStart(builder)
	// 	fbs.FileAddData(builder, filedata)
	// 	fileRecord := fbs.FileEnd(builder)
	// 	builder.FinishSizePrefixed(fileRecord)
	// 	log.Printf("%d Time to build Flatbuffer record: %v", i, time.Since(start))
	// 	start = time.Now()

	// 	_, err = c.Write(builder.FinishedBytes())
	// 	log.Printf("%d Time to write Flatbuffer record to UDS: %v", i, time.Since(start))
	// }

	// start = time.Now()
	// if err := task.Kill(ctx, syscall.SIGTERM); err != nil {
	// 	log.Fatal(err)
	// }
	// log.Printf("Time to kill task: %v", time.Since(start))
	// start = time.Now()

	// // wait for the process to fully exit and print out the exit status
	// start = time.Now()
	// status := <-exitStatusC
	// code, _, err := status.Result()
	// if err != nil {
	// 	log.Fatal(err)
	// }
	// fmt.Printf("reader exited with status: %d\n", code)
	// log.Printf("Time to finish task execution: %v", time.Since(start))
}

type IDDocument struct {
	ID uint64 `json:"id"`
}

func unzipSource(source, destination string) error {
	// 1. Open the zip file
	reader, err := zip.OpenReader(source)
	if err != nil {
		return err
	}
	defer reader.Close()

	// 2. Get the absolute destination path
	destination, err = filepath.Abs(destination)
	if err != nil {
		return err
	}

	// 3. Iterate over zip files inside the archive and unzip each of them
	for _, f := range reader.File {
		err := unzipFile(f, destination)
		if err != nil {
			return err
		}
	}

	return nil
}

func unzipFile(f *zip.File, destination string) error {
	// 4. Check if file paths are not vulnerable to Zip Slip
	filePath := filepath.Join(destination, f.Name)
	if !strings.HasPrefix(filePath, filepath.Clean(destination)+string(os.PathSeparator)) {
		return fmt.Errorf("invalid file path: %s", filePath)
	}

	// 5. Create directory tree
	if f.FileInfo().IsDir() {
		if err := os.MkdirAll(filePath, os.ModePerm); err != nil {
			return err
		}
		return nil
	}

	if err := os.MkdirAll(filepath.Dir(filePath), os.ModePerm); err != nil {
		return err
	}

	// 6. Create a destination file for unzipped content
	destinationFile, err := os.OpenFile(filePath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, f.Mode())
	if err != nil {
		return err
	}
	defer destinationFile.Close()

	// 7. Unzip the content of a file and copy it to the destination file
	zippedFile, err := f.Open()
	if err != nil {
		return err
	}
	defer zippedFile.Close()

	if _, err := io.Copy(destinationFile, zippedFile); err != nil {
		return err
	}
	return nil
}

func DownloadS3File(s3Path string) (string, error) {
	ctx := context.Background()
	file, err := os.Create("/tmp/images.zip")
	if err != nil {
		return "", err
	}

	defer file.Close()

	cfg, err := config.LoadDefaultConfig(ctx, config.WithRegion("us-west-2"))
	if err != nil {
		return "", err
	}

	objstore := objectstorage.NewAwsS3ObjectStore(ctx, cfg)
	_, err = objstore.DownloadObject(ctx, s3Path, file)

	if err != nil {
		return "", err
	}

	return file.Name(), nil
}

func main() {
	// Download and unzip test images
	localImagesZipPath, err := DownloadS3File(TestImagesZipS3Path)
	log.Print("done downloading images from s3")

	if err != nil {
		log.Fatal(err)
	}
	localImagesDirPath := "/tmp/images"
	err = unzipSource(localImagesZipPath, localImagesDirPath)
	if err != nil {
		log.Fatal(err)
	}

	launchReader(0, localImagesDirPath)

	r := mux.NewRouter()
	r.HandleFunc("/launch-reader", func(w http.ResponseWriter, req *http.Request) {
		decodedReq := new(IDDocument)
		json.NewDecoder(req.Body).Decode(&decodedReq)
		launchReader(int(decodedReq.ID), localImagesDirPath)
	}).Methods("POST")

	srv := &http.Server{
		Addr:    ":8080",
		Handler: r,
	}
	srv.ListenAndServe()
}
